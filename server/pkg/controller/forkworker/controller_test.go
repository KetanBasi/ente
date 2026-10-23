package forkworker

import (
	"crypto/rand"
	"database/sql"
	"encoding/base64"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/ente/museum/ente"
	fileData "github.com/ente/museum/ente/filedata"
	"github.com/ente/museum/ente/forkworker"
	"github.com/ente/museum/internal/testutil"
	"github.com/ente/museum/pkg/controller/access"
	"github.com/ente/museum/pkg/controller/filedata"
	"github.com/ente/museum/pkg/repo"
	workerRepo "github.com/ente/museum/pkg/repo/forkworker"
	publicRepo "github.com/ente/museum/pkg/repo/public"
	"github.com/gin-gonic/gin"
	"golang.org/x/crypto/nacl/box"
)

type workerFixture struct {
	pub  string
	priv *[32]byte
	pubK *[32]byte
}

func newWorkerKey(t *testing.T) workerFixture {
	t.Helper()
	pub, priv, err := box.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return workerFixture{pub: base64.StdEncoding.EncodeToString(pub[:]), priv: priv, pubK: pub}
}

// sealFileKey seals a random 32 byte fileKey the way a client will.
func (w workerFixture) sealFileKey(t *testing.T) string {
	t.Helper()
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	sealed, err := box.SealAnonymous(nil, key, w.pubK, rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return base64.StdEncoding.EncodeToString(sealed)
}

// openToken opens the pairing for userID the way the worker will.
func (w workerFixture) openToken(t *testing.T, c *Controller, userID int64) string {
	t.Helper()
	pairings, err := c.Pairings(t.Context(), w.pub)
	if err != nil {
		t.Fatalf("Pairings() error = %v", err)
	}
	for _, p := range pairings {
		if p.UserID != userID {
			continue
		}
		sealed, err := base64.StdEncoding.DecodeString(p.SealedToken)
		if err != nil {
			t.Fatal(err)
		}
		token, ok := box.OpenAnonymous(nil, sealed, w.pubK, w.priv)
		if !ok {
			t.Fatal("worker could not open its sealed token")
		}
		return string(token)
	}
	t.Fatalf("no pairing for user %d", userID)
	return ""
}

type testEnv struct {
	db     *sql.DB
	ctx    *gin.Context
	ctrl   *Controller
	worker workerFixture
	alice  int64
	bob    int64
}

func setup(t *testing.T) *testEnv {
	t.Helper()
	testutil.WithServerRoot(t)
	db := testutil.RequireTestDB(t)
	if err := workerRepo.MigrateUp(db, "file://migrations/fork"); err != nil {
		t.Fatalf("fork migrations: %v", err)
	}
	testutil.ResetTables(t, db)
	t.Cleanup(func() { testutil.ResetTables(t, db) })

	gin.SetMode(gin.TestMode)
	ctx, _ := gin.CreateTestContext(httptest.NewRecorder())
	ctx.Request = httptest.NewRequest(http.MethodGet, "/", nil)

	worker := newWorkerKey(t)
	keys, err := ValidateKeys([]forkworker.Key{{Name: "home-pc", PublicKey: worker.pub}})
	if err != nil {
		t.Fatal(err)
	}
	fileRepo := &repo.FileRepository{DB: db}
	accessCtrl := access.NewAccessController(&repo.CollectionRepository{
		DB:                 db,
		CollectionLinkRepo: publicRepo.NewCollectionLinkRepository(db, ""),
	}, fileRepo)
	ctrl := NewController(keys, &workerRepo.Repository{DB: db}, accessCtrl, fileRepo, nil, &filedata.Controller{FileRepo: fileRepo})
	return &testEnv{
		db: db, ctx: ctx, ctrl: ctrl, worker: worker,
		alice: testutil.InsertUser(t, db, testutil.UserFixture{Email: "alice@example.com", CreationTime: 1}),
		bob:   testutil.InsertUser(t, db, testutil.UserFixture{Email: "bob@example.com", CreationTime: 1}),
	}
}

func insertFile(t *testing.T, db *sql.DB, ownerID int64) int64 {
	t.Helper()
	var fileID int64
	err := db.QueryRow(`INSERT INTO files(owner_id, file_decryption_header, thumbnail_decryption_header, metadata_decryption_header, encrypted_metadata, updation_time, info)
		VALUES($1, 'file-header', 'thumb-header', 'meta-header', 'enc-meta', 1, '{}'::jsonb) RETURNING file_id`, ownerID).Scan(&fileID)
	if err != nil {
		t.Fatalf("insert file: %v", err)
	}
	return fileID
}

func requireDenied(t *testing.T, err error, what string) {
	t.Helper()
	if err == nil {
		t.Fatalf("%s: expected an error, got none", what)
	}
}

func (e *testEnv) enableAndAuth(t *testing.T, userID int64) forkworker.AuthContext {
	t.Helper()
	if _, err := e.ctrl.Enable(t.Context(), userID, e.worker.pub); err != nil {
		t.Fatalf("Enable() error = %v", err)
	}
	a, err := e.ctrl.Authenticate(t.Context(), e.worker.openToken(t, e.ctrl, userID))
	if err != nil {
		t.Fatalf("Authenticate() error = %v", err)
	}
	return *a
}

func TestEnableRejectsKeyNotOnAllowlist(t *testing.T) {
	e := setup(t)
	other := newWorkerKey(t)
	_, err := e.ctrl.Enable(t.Context(), e.alice, other.pub)
	requireDenied(t, err, "enable with unlisted key")
	_, err = e.ctrl.Pairings(t.Context(), other.pub)
	requireDenied(t, err, "pairings for unlisted key")
}

func TestTokenIsStoredOnlyHashedAndSealed(t *testing.T) {
	e := setup(t)
	a := e.enableAndAuth(t, e.alice)
	if a.UserID != e.alice || a.WorkerPublicKey != e.worker.pub {
		t.Fatalf("auth context = %+v", a)
	}
	token := e.worker.openToken(t, e.ctrl, e.alice)
	var n int
	if err := e.db.QueryRow(`SELECT COUNT(*) FROM fork_worker_tokens WHERE sealed_token = $1 OR token_hash = $2`, token, []byte(token)).Scan(&n); err != nil {
		t.Fatal(err)
	}
	if n != 0 {
		t.Fatal("raw token found in the database")
	}
}

func TestReEnableRotatesToken(t *testing.T) {
	e := setup(t)
	e.enableAndAuth(t, e.alice)
	oldToken := e.worker.openToken(t, e.ctrl, e.alice)
	e.enableAndAuth(t, e.alice)
	_, err := e.ctrl.Authenticate(t.Context(), oldToken)
	requireDenied(t, err, "old token after re-enable")
}

func TestReleaseKeysRules(t *testing.T) {
	e := setup(t)
	aliceFile := insertFile(t, e.db, e.alice)
	bobFile := insertFile(t, e.db, e.bob)
	req := func(fileID int64, sealed string) forkworker.ReleaseKeysRequest {
		return forkworker.ReleaseKeysRequest{WorkerPublicKey: e.worker.pub, Keys: []forkworker.SealedFileKey{{FileID: fileID, SealedFileKey: sealed}}}
	}

	_, err := e.ctrl.ReleaseKeys(e.ctx, e.alice, req(aliceFile, e.worker.sealFileKey(t)))
	requireDenied(t, err, "release before enabling")

	e.enableAndAuth(t, e.alice)
	_, err = e.ctrl.ReleaseKeys(e.ctx, e.alice, req(bobFile, e.worker.sealFileKey(t)))
	requireDenied(t, err, "release someone else's file")
	_, err = e.ctrl.ReleaseKeys(e.ctx, e.alice, req(aliceFile, base64.StdEncoding.EncodeToString(make([]byte, 32))))
	requireDenied(t, err, "release an unsealed 32 byte key")

	added, err := e.ctrl.ReleaseKeys(e.ctx, e.alice, req(aliceFile, e.worker.sealFileKey(t)))
	if err != nil || added != 1 {
		t.Fatalf("ReleaseKeys() = %d, %v", added, err)
	}
	added, err = e.ctrl.ReleaseKeys(e.ctx, e.alice, req(aliceFile, e.worker.sealFileKey(t)))
	if err != nil || added != 0 {
		t.Fatalf("second ReleaseKeys() = %d, %v; want 0 (kept first)", added, err)
	}
}

// The P2 gate: the worker token reaches only files it was sent keys for,
// and cannot edit or delete anything.
func TestWorkerScopeIsReleasedFilesOnly(t *testing.T) {
	e := setup(t)
	released := insertFile(t, e.db, e.alice)
	notReleased := insertFile(t, e.db, e.alice)
	bobFile := insertFile(t, e.db, e.bob)
	a := e.enableAndAuth(t, e.alice)
	if _, err := e.ctrl.ReleaseKeys(e.ctx, e.alice, forkworker.ReleaseKeysRequest{
		WorkerPublicKey: e.worker.pub,
		Keys:            []forkworker.SealedFileKey{{FileID: released, SealedFileKey: e.worker.sealFileKey(t)}},
	}); err != nil {
		t.Fatal(err)
	}

	file, err := e.ctrl.FileInfo(t.Context(), a, released)
	if err != nil || file.ID != released {
		t.Fatalf("FileInfo(released) = %+v, %v", file, err)
	}
	if file.EncryptedKey != "" {
		t.Fatal("file info leaked a collection-wrapped key")
	}
	for name, id := range map[string]int64{"unreleased": notReleased, "other user's": bobFile} {
		_, err := e.ctrl.FileInfo(t.Context(), a, id)
		requireDenied(t, err, "FileInfo of "+name+" file")
		_, err = e.ctrl.ObjectURL(e.ctx, a, id, ente.FILE)
		requireDenied(t, err, "download of "+name+" file")
		_, err = e.ctrl.GetMlData(e.ctx, a, id)
		requireDenied(t, err, "mldata read of "+name+" file")
		zero := int64(0)
		data, header := "x", "y"
		err = e.ctrl.PutMlData(e.ctx, a, &fileData.PutFileDataRequest{FileID: id, Type: ente.MlData, EncryptedData: &data, DecryptionHeader: &header, LastUpdatedAt: &zero})
		requireDenied(t, err, "mldata write of "+name+" file")
	}
	_, err = e.ctrl.GetMlDataBatch(e.ctx, a, []int64{released, notReleased})
	requireDenied(t, err, "batch mldata read with one unreleased file")

	data, header := "x", "y"
	err = e.ctrl.PutMlData(e.ctx, a, &fileData.PutFileDataRequest{FileID: released, Type: ente.MlData, EncryptedData: &data, DecryptionHeader: &header})
	var apiErr *ente.ApiError
	if !errors.As(err, &apiErr) || apiErr.HttpStatusCode != http.StatusBadRequest {
		t.Fatalf("mldata write without lastUpdatedAt: err = %v, want 400", err)
	}
	err = e.ctrl.PutMlData(e.ctx, a, &fileData.PutFileDataRequest{FileID: released, Type: ente.PreviewVideo, EncryptedData: &data, DecryptionHeader: &header})
	requireDenied(t, err, "write of a non-mldata type")

	// Bob's token, if Bob enabled the same worker, still can't see Alice's file.
	b := e.enableAndAuth(t, e.bob)
	_, err = e.ctrl.FileInfo(t.Context(), b, released)
	requireDenied(t, err, "another user's token on a released file")
}

func TestInboxConsumeAndKillSwitch(t *testing.T) {
	e := setup(t)
	f1 := insertFile(t, e.db, e.alice)
	f2 := insertFile(t, e.db, e.alice)
	a := e.enableAndAuth(t, e.alice)
	if _, err := e.ctrl.ReleaseKeys(e.ctx, e.alice, forkworker.ReleaseKeysRequest{
		WorkerPublicKey: e.worker.pub,
		Keys: []forkworker.SealedFileKey{
			{FileID: f1, SealedFileKey: e.worker.sealFileKey(t)},
			{FileID: f2, SealedFileKey: e.worker.sealFileKey(t)},
		},
	}); err != nil {
		t.Fatal(err)
	}
	items, err := e.ctrl.Pending(t.Context(), a, 0, 0)
	if err != nil || len(items) != 2 {
		t.Fatalf("Pending() = %d items, %v", len(items), err)
	}
	sealed, _ := base64.StdEncoding.DecodeString(items[0].SealedFileKey)
	if key, ok := box.OpenAnonymous(nil, sealed, e.worker.pubK, e.worker.priv); !ok || len(key) != 32 {
		t.Fatal("worker could not open a released fileKey")
	}
	if items, _ := e.ctrl.Pending(t.Context(), a, items[0].ID, 0); len(items) != 1 || items[0].FileID != f2 {
		t.Fatalf("Pending(sinceID) = %+v", items)
	}
	if n, err := e.ctrl.Consume(t.Context(), a, []int64{f1}); err != nil || n != 1 {
		t.Fatalf("Consume() = %d, %v", n, err)
	}
	if items, _ := e.ctrl.Pending(t.Context(), a, 0, 0); len(items) != 1 || items[0].FileID != f2 {
		t.Fatalf("Pending() after consume = %+v", items)
	}

	if err := e.ctrl.Disable(t.Context(), e.alice); err != nil {
		t.Fatal(err)
	}
	if e.worker.hasPairing(t, e.ctrl, e.alice) {
		t.Fatal("pairing still offered after disable")
	}
	log, err := e.ctrl.AuditLog(t.Context(), e.alice, 0, 0)
	if err != nil || len(log) != 1 || log[0].FileID != f1 || log[0].ConsumedAt == nil {
		t.Fatalf("audit log after disable = %+v, %v; want only the consumed f1", log, err)
	}
	statuses, err := e.ctrl.Status(t.Context(), e.alice)
	if err != nil || len(statuses) != 0 {
		t.Fatalf("Status() after disable = %+v, %v", statuses, err)
	}
}

func TestDisableRevokesToken(t *testing.T) {
	e := setup(t)
	e.enableAndAuth(t, e.alice)
	token := e.worker.openToken(t, e.ctrl, e.alice)
	if err := e.ctrl.Disable(t.Context(), e.alice); err != nil {
		t.Fatal(err)
	}
	_, err := e.ctrl.Authenticate(t.Context(), token)
	requireDenied(t, err, "token after disable")
}

func TestTokenDiesWhenKeyLeavesAllowlist(t *testing.T) {
	e := setup(t)
	e.enableAndAuth(t, e.alice)
	token := e.worker.openToken(t, e.ctrl, e.alice)
	other := newWorkerKey(t)
	keys, _ := ValidateKeys([]forkworker.Key{{Name: "new-pc", PublicKey: other.pub}})
	rotated := NewController(keys, e.ctrl.Repo, e.ctrl.AccessCtrl, e.ctrl.FileRepo, nil, e.ctrl.FileDataCtrl)
	_, err := rotated.Authenticate(t.Context(), token)
	requireDenied(t, err, "token after its key left the allowlist")
}

func (w workerFixture) hasPairing(t *testing.T, c *Controller, userID int64) bool {
	t.Helper()
	pairings, err := c.Pairings(t.Context(), w.pub)
	if err != nil {
		t.Fatal(err)
	}
	for _, p := range pairings {
		if p.UserID == userID {
			return true
		}
	}
	return false
}
