// Package forkworker holds the types for the fork's home AI worker: an
// allowlisted worker key, the read-only worker token, and the inbox of
// fileKeys that clients seal to that key. See fork/docs/worker-api.md.
package forkworker

import "github.com/google/uuid"

const (
	// TokenHeader carries the worker's read-only token on /fork/worker-api.
	TokenHeader = "X-Worker-Access-Token"
	// AuthContextKey is the gin context key the worker middleware sets.
	AuthContextKey = "X-Fork-Worker-Context"
	// PublicKeyBytes is the size of a raw X25519 public key.
	PublicKeyBytes = 32
	// SealedFileKeyBytes is crypto_box_seal of a 32 byte fileKey:
	// 32 byte ephemeral public key + 16 byte MAC + 32 byte key.
	SealedFileKeyBytes = 80
	// MaxInboxBatch caps the files in one inbox upload or consume call.
	MaxInboxBatch = 1000
	// MaxFileDataFetch caps one batch mldata read, as upstream does.
	MaxFileDataFetch = 200
)

// Key is one entry of the worker allowlist in museum config.
type Key struct {
	Name      string `json:"name" mapstructure:"name"`
	PublicKey string `json:"publicKey" mapstructure:"public-key"`
	// Fingerprint is hex SHA-256 of the raw public key, for display only.
	// Clients must compute it themselves before trusting a key.
	Fingerprint string `json:"fingerprint" mapstructure:"-"`
}

// AuthContext is what a valid worker token resolves to.
type AuthContext struct {
	TokenID         uuid.UUID
	UserID          int64
	WorkerPublicKey string
}

type EnableRequest struct {
	WorkerPublicKey string `json:"workerPublicKey" binding:"required"`
}

type Status struct {
	WorkerPublicKey string  `json:"workerPublicKey"`
	CreatedAt       int64   `json:"createdAt"`
	LastUsedAt      *int64  `json:"lastUsedAt"`
	PendingCount    int64   `json:"pendingCount"`
	ConsumedCount   int64   `json:"consumedCount"`
	TokenID         string  `json:"tokenID"`
	WorkerName      *string `json:"workerName,omitempty"`
}

// Pairing is a token sealed to a worker key. Only that worker can open it.
type Pairing struct {
	UserID      int64  `json:"userID"`
	SealedToken string `json:"sealedToken"`
	CreatedAt   int64  `json:"createdAt"`
}

type SealedFileKey struct {
	FileID        int64  `json:"fileID" binding:"required"`
	SealedFileKey string `json:"sealedFileKey" binding:"required"`
}

type ReleaseKeysRequest struct {
	WorkerPublicKey string          `json:"workerPublicKey" binding:"required"`
	Keys            []SealedFileKey `json:"keys" binding:"required"`
}

// InboxItem is one released fileKey as the worker sees it.
type InboxItem struct {
	ID            int64  `json:"id"`
	FileID        int64  `json:"fileID"`
	SealedFileKey string `json:"sealedFileKey"`
	CreatedAt     int64  `json:"createdAt"`
}

// AuditItem is one released fileKey as the owner sees it: no key bytes.
type AuditItem struct {
	ID              int64  `json:"id"`
	FileID          int64  `json:"fileID"`
	WorkerPublicKey string `json:"workerPublicKey"`
	CreatedAt       int64  `json:"createdAt"`
	ConsumedAt      *int64 `json:"consumedAt"`
}

type ConsumeRequest struct {
	FileIDs []int64 `json:"fileIDs" binding:"required"`
}
