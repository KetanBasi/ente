package filedata

// fork: mldata reads and writes on behalf of the home AI worker. The worker
// has no user session, so these take the file owner explicitly instead of
// reading the user from the request header. Callers must first check that
// the file was released to the worker (see pkg/controller/forkworker).

import (
	"context"

	"github.com/ente/museum/ente"
	fileData "github.com/ente/museum/ente/filedata"
	"github.com/ente/museum/pkg/utils/array"
	"github.com/ente/museum/pkg/utils/network"
	"github.com/ente/stacktrace"
	"github.com/gin-gonic/gin"
	log "github.com/sirupsen/logrus"
)

const forkWorkerClient = "fork-worker"

// WorkerGetMlData returns the file's mldata, or nil if it has none.
func (c *Controller) WorkerGetMlData(ctx *gin.Context, ownerID int64, fileID int64) (*fileData.Entity, error) {
	return c.GetFileData(ctx, ownerID, fileData.GetFileData{FileID: fileID, Type: ente.MlData, PreferNoContent: true})
}

// WorkerGetMlDataBatch is GetFilesData for mldata, for the file owner.
func (c *Controller) WorkerGetMlDataBatch(ctx *gin.Context, ownerID int64, fileIDs []int64) (*fileData.GetFilesDataResponse, error) {
	req := fileData.GetFilesData{FileIDs: fileIDs, Type: ente.MlData}
	if err := req.Validate(); err != nil {
		return nil, stacktrace.Propagate(err, "req validation failed")
	}
	if err := c._checkMetadataReadOrWritePerm(ctx, ownerID, fileIDs); err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	doRows, err := c.Repo.GetFilesData(ctx, req.Type, fileIDs)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	activeRows := make([]fileData.Row, 0)
	dbFileIds := make([]int64, 0)
	for i := range doRows {
		dbFileIds = append(dbFileIds, doRows[i].FileID)
		if !doRows[i].IsDeleted {
			activeRows = append(activeRows, doRows[i])
		}
	}
	results, err := c.getS3FileMetadataParallel(ctx, activeRows)
	if err != nil {
		return nil, stacktrace.Propagate(err, "")
	}
	resp := &fileData.GetFilesDataResponse{
		Data:                make([]fileData.Entity, 0),
		PendingIndexFileIDs: array.FindMissingElementsInSecondList(fileIDs, dbFileIds),
		ErrFileIDs:          make([]int64, 0),
	}
	for _, obj := range results {
		if obj.err != nil {
			resp.ErrFileIDs = append(resp.ErrFileIDs, obj.dbEntry.FileID)
			continue
		}
		resp.Data = append(resp.Data, fileData.Entity{
			FileID:           obj.dbEntry.FileID,
			Type:             obj.dbEntry.Type,
			EncryptedData:    obj.s3MetaObject.EncryptedData,
			DecryptionHeader: obj.s3MetaObject.DecryptionHeader,
			UpdatedAt:        obj.dbEntry.UpdatedAt,
		})
	}
	return resp, nil
}

// WorkerPutMlData is InsertOrUpdateMetadata for the file owner, with
// lastUpdatedAt required (0 for a file that has no mldata yet), so the
// worker can only replace the exact record it read and merged into.
func (c *Controller) WorkerPutMlData(ctx *gin.Context, ownerID int64, req *fileData.PutFileDataRequest) error {
	if err := req.Validate(); err != nil {
		return stacktrace.Propagate(err, "validation failed")
	}
	if req.Type != ente.MlData {
		return stacktrace.Propagate(ente.NewBadRequestWithMessage("unsupported object type "+string(req.Type)), "")
	}
	if req.LastUpdatedAt == nil {
		return stacktrace.Propagate(ente.NewBadRequestWithMessage("lastUpdatedAt is required"), "")
	}
	if req.Version == nil {
		version := 1
		req.Version = &version
	}
	fileOwnerID, err := c.FileRepo.GetOwnerID(req.FileID)
	if err != nil {
		return stacktrace.Propagate(err, "")
	}
	if fileOwnerID != ownerID {
		return stacktrace.Propagate(ente.ErrPermissionDenied, "worker may only write the owner's files")
	}
	if versionErr := c._validateLastUpdatedAt(ctx, req.LastUpdatedAt, req.FileID, req.Type); versionErr != nil {
		return stacktrace.Propagate(versionErr, "")
	}
	bucketID := c.S3Config.GetBucketID(req.Type)
	objectKey := fileData.ObjectMetadataKey(req.FileID, fileOwnerID, req.Type, nil)
	client := forkWorkerClient
	if info := network.GetClientInfo(ctx); info != "" {
		client = forkWorkerClient + " " + info
	}
	obj := fileData.S3FileMetadata{
		Version:          *req.Version,
		EncryptedData:    *req.EncryptedData,
		DecryptionHeader: *req.DecryptionHeader,
		Client:           client,
	}
	logger := log.WithField("objectKey", objectKey).WithField("fileID", req.FileID).WithField("type", req.Type).WithField("actor", forkWorkerClient)
	size, uploadErr := c.uploadObject(obj, objectKey, bucketID)
	if uploadErr != nil {
		logger.WithError(uploadErr).Error("upload failed")
		return uploadErr
	}
	row := fileData.Row{
		FileID:       req.FileID,
		Type:         req.Type,
		UserID:       fileOwnerID,
		Size:         size,
		LatestBucket: bucketID,
	}
	if err := c.Repo.InsertOrUpdate(context.Background(), row); err != nil {
		logger.WithError(err).Error("insert or update failed")
		return stacktrace.Propagate(err, "failed to insert or update file data row")
	}
	return nil
}
