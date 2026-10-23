package api

// fork: HTTP handlers for the home AI worker. Owner routes use the user's
// normal session; worker routes use the read-only worker token. See
// fork/docs/worker-api.md.

import (
	"net/http"
	"strconv"

	"github.com/ente/museum/ente"
	fileData "github.com/ente/museum/ente/filedata"
	"github.com/ente/museum/ente/forkworker"
	workerCtrl "github.com/ente/museum/pkg/controller/forkworker"
	"github.com/ente/museum/pkg/utils/auth"
	"github.com/ente/museum/pkg/utils/handler"
	"github.com/ente/stacktrace"
	"github.com/gin-gonic/gin"
)

type ForkWorkerHandler struct {
	Ctrl *workerCtrl.Controller
}

// RegisterForkWorkerRoutes wires every fork worker route. Owner routes go on
// storageAPI (user session), pairing on publicAPI, and the worker's own
// routes on workerAPI, which must carry only the worker token middleware.
// The worker token can therefore reach nothing but the routes listed here.
func RegisterForkWorkerRoutes(publicAPI, storageAPI, workerAPI gin.IRoutes, h *ForkWorkerHandler) {
	storageAPI.GET("/fork/worker/keys", h.GetKeys)
	storageAPI.GET("/fork/worker/status", h.GetStatus)
	storageAPI.POST("/fork/worker/token", h.Enable)
	storageAPI.DELETE("/fork/worker/token", h.Disable)
	storageAPI.POST("/fork/worker/inbox", h.ReleaseKeys)
	storageAPI.GET("/fork/worker/inbox", h.GetAuditLog)

	publicAPI.GET("/fork/worker/pairing", h.GetPairings)

	workerAPI.GET("/inbox", h.GetInbox)
	workerAPI.POST("/inbox/consume", h.ConsumeInbox)
	workerAPI.GET("/file-info/:fileID", h.GetFileInfo)
	workerAPI.GET("/files/download/:fileID", h.redirect(ente.FILE))
	workerAPI.GET("/files/preview/:fileID", h.redirect(ente.THUMBNAIL))
	workerAPI.GET("/files/download/v3/:fileID", h.urlV3(ente.FILE))
	workerAPI.GET("/files/thumbnail/v3/:fileID", h.urlV3(ente.THUMBNAIL))
	workerAPI.GET("/files/data", h.GetMlData)
	workerAPI.POST("/files/data/fetch", h.GetMlDataBatch)
	workerAPI.PUT("/files/data", h.PutMlData)
}

func (h *ForkWorkerHandler) GetKeys(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"keys": h.Ctrl.Keys()})
}

func (h *ForkWorkerHandler) GetStatus(c *gin.Context) {
	statuses, err := h.Ctrl.Status(c, auth.GetUserID(c.Request.Header))
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{"workers": statuses})
}

func (h *ForkWorkerHandler) Enable(c *gin.Context) {
	var req forkworker.EnableRequest
	if err := handler.BindJSON(c, &req); err != nil {
		handler.Error(c, ente.NewBadRequestWithMessage(err.Error()))
		return
	}
	status, err := h.Ctrl.Enable(c, auth.GetUserID(c.Request.Header), req.WorkerPublicKey)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, "failed to enable worker"))
		return
	}
	c.JSON(http.StatusOK, status)
}

func (h *ForkWorkerHandler) Disable(c *gin.Context) {
	if err := h.Ctrl.Disable(c, auth.GetUserID(c.Request.Header)); err != nil {
		handler.Error(c, stacktrace.Propagate(err, "failed to disable worker"))
		return
	}
	c.JSON(http.StatusOK, gin.H{})
}

func (h *ForkWorkerHandler) ReleaseKeys(c *gin.Context) {
	var req forkworker.ReleaseKeysRequest
	if err := handler.BindJSON(c, &req); err != nil {
		handler.Error(c, ente.NewBadRequestWithMessage(err.Error()))
		return
	}
	added, err := h.Ctrl.ReleaseKeys(c, auth.GetUserID(c.Request.Header), req)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, "failed to release keys"))
		return
	}
	c.JSON(http.StatusOK, gin.H{"added": added})
}

func (h *ForkWorkerHandler) GetAuditLog(c *gin.Context) {
	sinceID, limit, err := pageParams(c)
	if err != nil {
		handler.Error(c, err)
		return
	}
	items, err := h.Ctrl.AuditLog(c, auth.GetUserID(c.Request.Header), sinceID, limit)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{"items": items})
}

func (h *ForkWorkerHandler) GetPairings(c *gin.Context) {
	pairings, err := h.Ctrl.Pairings(c, c.Query("workerPublicKey"))
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{"pairings": pairings})
}

func (h *ForkWorkerHandler) GetInbox(c *gin.Context) {
	sinceID, limit, err := pageParams(c)
	if err != nil {
		handler.Error(c, err)
		return
	}
	items, err := h.Ctrl.Pending(c, workerCtx(c), sinceID, limit)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{"items": items})
}

func (h *ForkWorkerHandler) ConsumeInbox(c *gin.Context) {
	var req forkworker.ConsumeRequest
	if err := handler.BindJSON(c, &req); err != nil {
		handler.Error(c, ente.NewBadRequestWithMessage(err.Error()))
		return
	}
	n, err := h.Ctrl.Consume(c, workerCtx(c), req.FileIDs)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{"consumed": n})
}

func (h *ForkWorkerHandler) GetFileInfo(c *gin.Context) {
	fileID, err := strconv.ParseInt(c.Param("fileID"), 10, 64)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(ente.ErrBadRequest, ""))
		return
	}
	file, err := h.Ctrl.FileInfo(c, workerCtx(c), fileID)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{"file": file})
}

func (h *ForkWorkerHandler) redirect(objectType ente.ObjectType) gin.HandlerFunc {
	return func(c *gin.Context) {
		url, err := h.objectURL(c, objectType)
		if err != nil {
			handler.Error(c, stacktrace.Propagate(err, ""))
			return
		}
		c.Redirect(http.StatusTemporaryRedirect, url)
	}
}

func (h *ForkWorkerHandler) urlV3(objectType ente.ObjectType) gin.HandlerFunc {
	return func(c *gin.Context) {
		url, err := h.objectURL(c, objectType)
		writeFileURLV3(c, url, err)
	}
}

func (h *ForkWorkerHandler) objectURL(c *gin.Context, objectType ente.ObjectType) (string, error) {
	fileID, err := strconv.ParseInt(c.Param("fileID"), 10, 64)
	if err != nil {
		return "", stacktrace.Propagate(ente.ErrBadRequest, "")
	}
	return h.Ctrl.ObjectURL(c, workerCtx(c), fileID, objectType)
}

func (h *ForkWorkerHandler) GetMlData(c *gin.Context) {
	fileID, err := strconv.ParseInt(c.Query("fileID"), 10, 64)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(ente.ErrBadRequest, "fileID is required"))
		return
	}
	data, err := h.Ctrl.GetMlData(c, workerCtx(c), fileID)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	if data == nil {
		c.Status(http.StatusNoContent)
		return
	}
	c.JSON(http.StatusOK, gin.H{"data": data})
}

func (h *ForkWorkerHandler) GetMlDataBatch(c *gin.Context) {
	var req forkworker.ConsumeRequest
	if err := handler.BindJSON(c, &req); err != nil {
		handler.Error(c, ente.NewBadRequestWithMessage(err.Error()))
		return
	}
	resp, err := h.Ctrl.GetMlDataBatch(c, workerCtx(c), req.FileIDs)
	if err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, resp)
}

func (h *ForkWorkerHandler) PutMlData(c *gin.Context) {
	var req fileData.PutFileDataRequest
	if err := handler.BindJSON(c, &req); err != nil {
		handler.Error(c, ente.NewBadRequestWithMessage(err.Error()))
		return
	}
	if err := h.Ctrl.PutMlData(c, workerCtx(c), &req); err != nil {
		handler.Error(c, stacktrace.Propagate(err, ""))
		return
	}
	c.JSON(http.StatusOK, gin.H{})
}

func workerCtx(c *gin.Context) forkworker.AuthContext {
	return c.MustGet(forkworker.AuthContextKey).(forkworker.AuthContext)
}

func pageParams(c *gin.Context) (int64, int, error) {
	var sinceID int64
	var limit int
	var err error
	if s := c.Query("sinceID"); s != "" {
		if sinceID, err = strconv.ParseInt(s, 10, 64); err != nil || sinceID < 0 {
			return 0, 0, stacktrace.Propagate(ente.NewBadRequestWithMessage("invalid sinceID"), "")
		}
	}
	if s := c.Query("limit"); s != "" {
		if limit, err = strconv.Atoi(s); err != nil || limit < 0 {
			return 0, 0, stacktrace.Propagate(ente.NewBadRequestWithMessage("invalid limit"), "")
		}
	}
	return sinceID, limit, nil
}
