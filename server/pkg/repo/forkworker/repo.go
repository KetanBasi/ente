package forkworker

import (
	"context"
	"database/sql"

	"github.com/ente/museum/ente"
	"github.com/ente/museum/ente/forkworker"
	"github.com/ente/stacktrace"
	"github.com/google/uuid"
	"github.com/lib/pq"
)

type Repository struct {
	DB *sql.DB
}

// ReplaceToken revokes any live token of this user for this worker and
// stores a new one, in one transaction.
func (r *Repository) ReplaceToken(ctx context.Context, userID int64, workerPublicKey string, tokenHash []byte, sealedToken string) (uuid.UUID, error) {
	tx, err := r.DB.BeginTx(ctx, nil)
	if err != nil {
		return uuid.Nil, stacktrace.Propagate(err, "")
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `UPDATE fork_worker_tokens SET revoked_at = now_utc_micro_seconds()
		WHERE user_id = $1 AND worker_public_key = $2 AND revoked_at IS NULL`, userID, workerPublicKey); err != nil {
		return uuid.Nil, stacktrace.Propagate(err, "failed to revoke old token")
	}
	id := uuid.New()
	if _, err := tx.ExecContext(ctx, `INSERT INTO fork_worker_tokens (id, user_id, worker_public_key, token_hash, sealed_token)
		VALUES ($1, $2, $3, $4, $5)`, id, userID, workerPublicKey, tokenHash, sealedToken); err != nil {
		return uuid.Nil, stacktrace.Propagate(err, "failed to insert token")
	}
	return id, stacktrace.Propagate(tx.Commit(), "")
}

// RevokeAll revokes every live token of the user and drops the fileKeys the
// worker has not read yet. Consumed rows stay as the audit log.
func (r *Repository) RevokeAll(ctx context.Context, userID int64) error {
	tx, err := r.DB.BeginTx(ctx, nil)
	if err != nil {
		return stacktrace.Propagate(err, "")
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `UPDATE fork_worker_tokens SET revoked_at = now_utc_micro_seconds()
		WHERE user_id = $1 AND revoked_at IS NULL`, userID); err != nil {
		return stacktrace.Propagate(err, "failed to revoke tokens")
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM fork_worker_inbox WHERE owner_id = $1 AND consumed_at IS NULL`, userID); err != nil {
		return stacktrace.Propagate(err, "failed to drop unread keys")
	}
	return stacktrace.Propagate(tx.Commit(), "")
}

func (r *Repository) GetAuthContext(ctx context.Context, tokenHash []byte) (*forkworker.AuthContext, error) {
	var res forkworker.AuthContext
	err := r.DB.QueryRowContext(ctx, `SELECT id, user_id, worker_public_key FROM fork_worker_tokens
		WHERE token_hash = $1 AND revoked_at IS NULL`, tokenHash).Scan(&res.TokenID, &res.UserID, &res.WorkerPublicKey)
	if err == sql.ErrNoRows {
		return nil, stacktrace.Propagate(ente.ErrPermissionDenied, "invalid worker token")
	}
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	return &res, nil
}

func (r *Repository) UpdateLastUsedAt(ctx context.Context, tokenID uuid.UUID) error {
	_, err := r.DB.ExecContext(ctx, `UPDATE fork_worker_tokens SET last_used_at = now_utc_micro_seconds() WHERE id = $1`, tokenID)
	return stacktrace.Propagate(err, "")
}

func (r *Repository) HasLiveToken(ctx context.Context, userID int64, workerPublicKey string) (bool, error) {
	var exists bool
	err := r.DB.QueryRowContext(ctx, `SELECT EXISTS (SELECT 1 FROM fork_worker_tokens
		WHERE user_id = $1 AND worker_public_key = $2 AND revoked_at IS NULL)`, userID, workerPublicKey).Scan(&exists)
	return exists, stacktrace.Propagate(err, "")
}

// GetStatus lists the user's live tokens with inbox counts.
func (r *Repository) GetStatus(ctx context.Context, userID int64) ([]forkworker.Status, error) {
	rows, err := r.DB.QueryContext(ctx, `SELECT t.id, t.worker_public_key, t.created_at, t.last_used_at,
			COUNT(i.id) FILTER (WHERE i.consumed_at IS NULL),
			COUNT(i.id) FILTER (WHERE i.consumed_at IS NOT NULL)
		FROM fork_worker_tokens t
		LEFT JOIN fork_worker_inbox i ON i.owner_id = t.user_id AND i.worker_public_key = t.worker_public_key
		WHERE t.user_id = $1 AND t.revoked_at IS NULL
		GROUP BY t.id ORDER BY t.created_at`, userID)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	defer rows.Close()
	res := make([]forkworker.Status, 0)
	for rows.Next() {
		var s forkworker.Status
		var id uuid.UUID
		if err := rows.Scan(&id, &s.WorkerPublicKey, &s.CreatedAt, &s.LastUsedAt, &s.PendingCount, &s.ConsumedCount); err != nil {
			return nil, stacktrace.Propagate(err, "")
		}
		s.TokenID = id.String()
		res = append(res, s)
	}
	return res, stacktrace.Propagate(rows.Err(), "")
}

// GetPairings returns the live sealed tokens for a worker key.
func (r *Repository) GetPairings(ctx context.Context, workerPublicKey string) ([]forkworker.Pairing, error) {
	rows, err := r.DB.QueryContext(ctx, `SELECT user_id, sealed_token, created_at FROM fork_worker_tokens
		WHERE worker_public_key = $1 AND revoked_at IS NULL ORDER BY created_at`, workerPublicKey)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	defer rows.Close()
	res := make([]forkworker.Pairing, 0)
	for rows.Next() {
		var p forkworker.Pairing
		if err := rows.Scan(&p.UserID, &p.SealedToken, &p.CreatedAt); err != nil {
			return nil, stacktrace.Propagate(err, "")
		}
		res = append(res, p)
	}
	return res, stacktrace.Propagate(rows.Err(), "")
}

// AddSealedKeys stores released fileKeys. A file already released to this
// worker keeps its first row, so a consumed key is not re-queued.
func (r *Repository) AddSealedKeys(ctx context.Context, ownerID int64, workerPublicKey string, keys []forkworker.SealedFileKey) (int64, error) {
	fileIDs := make([]int64, len(keys))
	sealed := make([]string, len(keys))
	for i := range keys {
		fileIDs[i] = keys[i].FileID
		sealed[i] = keys[i].SealedFileKey
	}
	res, err := r.DB.ExecContext(ctx, `INSERT INTO fork_worker_inbox (file_id, owner_id, worker_public_key, sealed_file_key)
		SELECT f, $1, $2, k FROM UNNEST($3::BIGINT[], $4::TEXT[]) AS t(f, k)
		ON CONFLICT (file_id, worker_public_key) DO NOTHING`, ownerID, workerPublicKey, pq.Array(fileIDs), pq.Array(sealed))
	if err != nil {
		return 0, stacktrace.Propagate(err, "failed to insert sealed keys")
	}
	n, err := res.RowsAffected()
	return n, stacktrace.Propagate(err, "")
}

// GetPending returns unread fileKeys with id > sinceID, oldest first.
func (r *Repository) GetPending(ctx context.Context, ownerID int64, workerPublicKey string, sinceID int64, limit int) ([]forkworker.InboxItem, error) {
	rows, err := r.DB.QueryContext(ctx, `SELECT id, file_id, sealed_file_key, created_at FROM fork_worker_inbox
		WHERE owner_id = $1 AND worker_public_key = $2 AND consumed_at IS NULL AND id > $3
		ORDER BY id LIMIT $4`, ownerID, workerPublicKey, sinceID, limit)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	defer rows.Close()
	res := make([]forkworker.InboxItem, 0)
	for rows.Next() {
		var item forkworker.InboxItem
		if err := rows.Scan(&item.ID, &item.FileID, &item.SealedFileKey, &item.CreatedAt); err != nil {
			return nil, stacktrace.Propagate(err, "")
		}
		res = append(res, item)
	}
	return res, stacktrace.Propagate(rows.Err(), "")
}

// GetAuditLog returns released fileKeys (without key bytes), oldest first.
func (r *Repository) GetAuditLog(ctx context.Context, ownerID int64, sinceID int64, limit int) ([]forkworker.AuditItem, error) {
	rows, err := r.DB.QueryContext(ctx, `SELECT id, file_id, worker_public_key, created_at, consumed_at FROM fork_worker_inbox
		WHERE owner_id = $1 AND id > $2 ORDER BY id LIMIT $3`, ownerID, sinceID, limit)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	defer rows.Close()
	res := make([]forkworker.AuditItem, 0)
	for rows.Next() {
		var item forkworker.AuditItem
		if err := rows.Scan(&item.ID, &item.FileID, &item.WorkerPublicKey, &item.CreatedAt, &item.ConsumedAt); err != nil {
			return nil, stacktrace.Propagate(err, "")
		}
		res = append(res, item)
	}
	return res, stacktrace.Propagate(rows.Err(), "")
}

// MarkConsumed records that the worker has read these fileKeys.
func (r *Repository) MarkConsumed(ctx context.Context, ownerID int64, workerPublicKey string, fileIDs []int64) (int64, error) {
	res, err := r.DB.ExecContext(ctx, `UPDATE fork_worker_inbox SET consumed_at = now_utc_micro_seconds()
		WHERE owner_id = $1 AND worker_public_key = $2 AND file_id = ANY($3) AND consumed_at IS NULL`,
		ownerID, workerPublicKey, pq.Array(fileIDs))
	if err != nil {
		return 0, stacktrace.Propagate(err, "")
	}
	n, err := res.RowsAffected()
	return n, stacktrace.Propagate(err, "")
}

// CountReleased returns how many of fileIDs were released to this worker by
// this owner, consumed or not. The worker may touch only those files.
func (r *Repository) CountReleased(ctx context.Context, ownerID int64, workerPublicKey string, fileIDs []int64) (int, error) {
	var n int
	err := r.DB.QueryRowContext(ctx, `SELECT COUNT(DISTINCT file_id) FROM fork_worker_inbox
		WHERE owner_id = $1 AND worker_public_key = $2 AND file_id = ANY($3)`,
		ownerID, workerPublicKey, pq.Array(fileIDs)).Scan(&n)
	return n, stacktrace.Propagate(err, "")
}
