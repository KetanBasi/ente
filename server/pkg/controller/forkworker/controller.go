// Package forkworker lets one allowlisted home worker analyse a user's
// library: clients seal fileKeys to the worker's public key into an inbox,
// and the worker reads them with a read-only token modelled on Cast.
//
// The worker token can read the files it was sent keys for and write their
// mldata. It cannot reach any other route: it is a different header from
// the user's auth token, and only the /fork/worker-api group accepts it.
package forkworker

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"fmt"

	"github.com/ente/museum/ente"
	fileData "github.com/ente/museum/ente/filedata"
	"github.com/ente/museum/ente/forkworker"
	"github.com/ente/museum/pkg/controller"
	"github.com/ente/museum/pkg/controller/access"
	"github.com/ente/museum/pkg/controller/filedata"
	"github.com/ente/museum/pkg/repo"
	workerRepo "github.com/ente/museum/pkg/repo/forkworker"
	enteArray "github.com/ente/museum/pkg/utils/array"
	"github.com/ente/stacktrace"
	"github.com/gin-gonic/gin"
	log "github.com/sirupsen/logrus"
	"golang.org/x/crypto/nacl/box"
)

const (
	tokenBytes      = 32
	maxInboxPage    = 1000
	defaultPageSize = 500
)

type Controller struct {
	Repo         *workerRepo.Repository
	AccessCtrl   access.Controller
	FileRepo     *repo.FileRepository
	FileCtrl     *controller.FileController
	FileDataCtrl *filedata.Controller
	keys         map[string]forkworker.Key
	keyList      []forkworker.Key
}

func NewController(keys []forkworker.Key, r *workerRepo.Repository, accessCtrl access.Controller,
	fileRepo *repo.FileRepository, fileCtrl *controller.FileController, fileDataCtrl *filedata.Controller,
) *Controller {
	m := make(map[string]forkworker.Key, len(keys))
	for _, k := range keys {
		m[k.PublicKey] = k
	}
	return &Controller{Repo: r, AccessCtrl: accessCtrl, FileRepo: fileRepo, FileCtrl: fileCtrl,
		FileDataCtrl: fileDataCtrl, keys: m, keyList: keys}
}

// Keys returns the allowlist.
func (c *Controller) Keys() []forkworker.Key {
	return c.keyList
}

func (c *Controller) allowed(workerPublicKey string) (forkworker.Key, error) {
	k, ok := c.keys[workerPublicKey]
	if !ok {
		return k, stacktrace.Propagate(ente.NewPermissionDeniedError("worker key is not on the allowlist"), "")
	}
	return k, nil
}

// Enable issues a new worker token for the user (revoking any older one for
// the same worker) and stores it sealed to the worker's public key. Nobody
// but the worker ever sees the raw token, the user included.
func (c *Controller) Enable(ctx context.Context, userID int64, workerPublicKey string) (*forkworker.Status, error) {
	if _, err := c.allowed(workerPublicKey); err != nil {
		return nil, err
	}
	var recipient [forkworker.PublicKeyBytes]byte
	raw, err := base64.StdEncoding.DecodeString(workerPublicKey)
	if err != nil || len(raw) != len(recipient) {
		return nil, stacktrace.Propagate(ente.ErrBadRequest, "invalid worker public key")
	}
	copy(recipient[:], raw)
	secret := make([]byte, tokenBytes)
	if _, err := rand.Read(secret); err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	token := base64.URLEncoding.EncodeToString(secret)
	sealed, err := box.SealAnonymous(nil, []byte(token), &recipient, rand.Reader)
	if err != nil {
		return nil, stacktrace.Propagate(err, "failed to seal token")
	}
	hash := HashToken(token)
	if _, err := c.Repo.ReplaceToken(ctx, userID, workerPublicKey, hash, base64.StdEncoding.EncodeToString(sealed)); err != nil {
		return nil, err
	}
	statuses, err := c.Status(ctx, userID)
	if err != nil {
		return nil, err
	}
	for i := range statuses {
		if statuses[i].WorkerPublicKey == workerPublicKey {
			return &statuses[i], nil
		}
	}
	return nil, stacktrace.NewError("token missing after enable")
}

// Disable is the kill switch: it revokes every worker token of the user and
// drops fileKeys the worker has not read yet.
func (c *Controller) Disable(ctx context.Context, userID int64) error {
	return c.Repo.RevokeAll(ctx, userID)
}

func (c *Controller) Status(ctx context.Context, userID int64) ([]forkworker.Status, error) {
	statuses, err := c.Repo.GetStatus(ctx, userID)
	if err != nil {
		return nil, err
	}
	for i := range statuses {
		if k, ok := c.keys[statuses[i].WorkerPublicKey]; ok {
			name := k.Name
			statuses[i].WorkerName = &name
		}
	}
	return statuses, nil
}

// Pairings returns the tokens sealed to an allowlisted worker key. It is
// public: only the holder of the worker's private key can open them.
func (c *Controller) Pairings(ctx context.Context, workerPublicKey string) ([]forkworker.Pairing, error) {
	if _, err := c.allowed(workerPublicKey); err != nil {
		return nil, err
	}
	return c.Repo.GetPairings(ctx, workerPublicKey)
}

// ReleaseKeys stores fileKeys a client sealed to the worker. Only the owner
// can release a file's key, and only while the worker is enabled.
func (c *Controller) ReleaseKeys(ctx *gin.Context, userID int64, req forkworker.ReleaseKeysRequest) (int64, error) {
	if _, err := c.allowed(req.WorkerPublicKey); err != nil {
		return 0, err
	}
	if len(req.Keys) == 0 || len(req.Keys) > forkworker.MaxInboxBatch {
		return 0, stacktrace.Propagate(ente.NewBadRequestWithMessage(fmt.Sprintf("keys must have 1 to %d entries", forkworker.MaxInboxBatch)), "")
	}
	fileIDs := make([]int64, len(req.Keys))
	for i, k := range req.Keys {
		raw, err := base64.StdEncoding.Strict().DecodeString(k.SealedFileKey)
		if err != nil || len(raw) != forkworker.SealedFileKeyBytes {
			return 0, stacktrace.Propagate(ente.NewBadRequestWithMessage(fmt.Sprintf("file %d: sealedFileKey must be %d bytes of base64", k.FileID, forkworker.SealedFileKeyBytes)), "")
		}
		fileIDs[i] = k.FileID
	}
	enabled, err := c.Repo.HasLiveToken(ctx, userID, req.WorkerPublicKey)
	if err != nil {
		return 0, err
	}
	if !enabled {
		return 0, stacktrace.Propagate(ente.NewPermissionDeniedError("worker is not enabled"), "")
	}
	if err := c.AccessCtrl.VerifyFileOwnership(ctx, &access.VerifyFileOwnershipParams{ActorUserId: userID, FileIDs: fileIDs}); err != nil {
		return 0, stacktrace.Propagate(err, "")
	}
	return c.Repo.AddSealedKeys(ctx, userID, req.WorkerPublicKey, req.Keys)
}

func (c *Controller) AuditLog(ctx context.Context, userID int64, sinceID int64, limit int) ([]forkworker.AuditItem, error) {
	return c.Repo.GetAuditLog(ctx, userID, sinceID, pageSize(limit))
}

// Authenticate resolves a worker token. A token stops working as soon as its
// worker key leaves the allowlist, even before anyone revokes it.
func (c *Controller) Authenticate(ctx context.Context, token string) (*forkworker.AuthContext, error) {
	authCtx, err := c.Repo.GetAuthContext(ctx, HashToken(token))
	if err != nil {
		return nil, err
	}
	if _, err := c.allowed(authCtx.WorkerPublicKey); err != nil {
		return nil, err
	}
	go func() {
		if err := c.Repo.UpdateLastUsedAt(context.Background(), authCtx.TokenID); err != nil {
			log.WithError(err).Warn("fork worker: failed to update last used")
		}
	}()
	return authCtx, nil
}

func (c *Controller) Pending(ctx context.Context, a forkworker.AuthContext, sinceID int64, limit int) ([]forkworker.InboxItem, error) {
	return c.Repo.GetPending(ctx, a.UserID, a.WorkerPublicKey, sinceID, pageSize(limit))
}

func (c *Controller) Consume(ctx context.Context, a forkworker.AuthContext, fileIDs []int64) (int64, error) {
	if len(fileIDs) == 0 || len(fileIDs) > forkworker.MaxInboxBatch {
		return 0, stacktrace.Propagate(ente.NewBadRequestWithMessage(fmt.Sprintf("fileIDs must have 1 to %d entries", forkworker.MaxInboxBatch)), "")
	}
	return c.Repo.MarkConsumed(ctx, a.UserID, a.WorkerPublicKey, fileIDs)
}

// requireReleased fails unless every file was released to this worker by
// the token's user. This is the worker's whole access scope.
func (c *Controller) requireReleased(ctx context.Context, a forkworker.AuthContext, fileIDs []int64) error {
	if len(fileIDs) == 0 {
		return stacktrace.Propagate(ente.ErrBadRequest, "no fileIDs")
	}
	if enteArray.ContainsDuplicateInInt64Array(fileIDs) {
		return stacktrace.Propagate(ente.ErrBadRequest, "duplicate fileIDs")
	}
	n, err := c.Repo.CountReleased(ctx, a.UserID, a.WorkerPublicKey, fileIDs)
	if err != nil {
		return err
	}
	if n != len(fileIDs) {
		return stacktrace.Propagate(ente.NewPermissionDeniedError("file key was not released to this worker"), "")
	}
	return nil
}

// FileInfo returns the decryption headers, encrypted metadata and public
// magic metadata of a released file. No collection-wrapped key is included.
func (c *Controller) FileInfo(ctx context.Context, a forkworker.AuthContext, fileID int64) (*ente.File, error) {
	if err := c.requireReleased(ctx, a, []int64{fileID}); err != nil {
		return nil, err
	}
	file, err := c.FileRepo.GetFileAttributes(fileID)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	if file.OwnerID != a.UserID {
		return nil, stacktrace.Propagate(ente.ErrPermissionDenied, "file owner changed")
	}
	return file, nil
}

func (c *Controller) ObjectURL(ctx *gin.Context, a forkworker.AuthContext, fileID int64, objectType ente.ObjectType) (string, error) {
	if err := c.requireReleased(ctx, a, []int64{fileID}); err != nil {
		return "", err
	}
	switch objectType {
	case ente.FILE:
		return c.FileCtrl.GetFileURLForOwner(ctx, a.UserID, fileID)
	case ente.THUMBNAIL:
		return c.FileCtrl.GetThumbnailURLForOwner(ctx, a.UserID, fileID)
	}
	return "", stacktrace.Propagate(ente.ErrBadRequest, "unsupported object type")
}

func (c *Controller) GetMlData(ctx *gin.Context, a forkworker.AuthContext, fileID int64) (*fileData.Entity, error) {
	if err := c.requireReleased(ctx, a, []int64{fileID}); err != nil {
		return nil, err
	}
	return c.FileDataCtrl.WorkerGetMlData(ctx, a.UserID, fileID)
}

func (c *Controller) GetMlDataBatch(ctx *gin.Context, a forkworker.AuthContext, fileIDs []int64) (*fileData.GetFilesDataResponse, error) {
	if len(fileIDs) > forkworker.MaxFileDataFetch {
		return nil, stacktrace.Propagate(ente.NewBadRequestWithMessage(fmt.Sprintf("at most %d fileIDs", forkworker.MaxFileDataFetch)), "")
	}
	if err := c.requireReleased(ctx, a, fileIDs); err != nil {
		return nil, err
	}
	return c.FileDataCtrl.WorkerGetMlDataBatch(ctx, a.UserID, fileIDs)
}

// PutMlData writes a released file's mldata. The record is encrypted with
// the fileKey, so the server cannot see which keys inside it changed; the
// worker must merge its x_enrich key into the record it read. What the
// server does enforce: mldata only, released files only, and a mandatory
// lastUpdatedAt so the worker can never blindly overwrite a newer record.
func (c *Controller) PutMlData(ctx *gin.Context, a forkworker.AuthContext, req *fileData.PutFileDataRequest) error {
	if err := c.requireReleased(ctx, a, []int64{req.FileID}); err != nil {
		return err
	}
	return c.FileDataCtrl.WorkerPutMlData(ctx, a.UserID, req)
}

func HashToken(token string) []byte {
	sum := sha256.Sum256([]byte(token))
	return sum[:]
}

func pageSize(limit int) int {
	if limit <= 0 {
		return defaultPageSize
	}
	if limit > maxInboxPage {
		return maxInboxPage
	}
	return limit
}
