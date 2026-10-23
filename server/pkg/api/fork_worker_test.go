package api

import (
	"net/http"
	"net/http/httptest"
	"sort"
	"strings"
	"testing"

	"github.com/ente/museum/ente/forkworker"
	"github.com/ente/museum/pkg/middleware"
	"github.com/gin-gonic/gin"
)

// The full list of what a worker token can reach. Adding a route here is a
// security decision: it widens what the worker can do.
var wantWorkerRoutes = []string{
	"GET /fork/worker-api/file-info/:fileID",
	"GET /fork/worker-api/files/data",
	"GET /fork/worker-api/files/download/:fileID",
	"GET /fork/worker-api/files/download/v3/:fileID",
	"GET /fork/worker-api/files/preview/:fileID",
	"GET /fork/worker-api/files/thumbnail/v3/:fileID",
	"GET /fork/worker-api/inbox",
	"POST /fork/worker-api/files/data/fetch", // read: a batch of mldata
	"POST /fork/worker-api/inbox/consume",    // marks its own inbox rows read
	"PUT /fork/worker-api/files/data",        // mldata only, released files only
}

func forkWorkerTestRouter() *gin.Engine {
	gin.SetMode(gin.TestMode)
	r := gin.New()
	public := r.Group("/")
	storage := r.Group("/")
	// Stands in for the user session middleware: the worker header is not
	// a user session.
	storage.Use(func(c *gin.Context) {
		if c.GetHeader("X-Auth-Token") == "" {
			c.AbortWithStatus(http.StatusUnauthorized)
		}
	})
	worker := r.Group("/fork/worker-api")
	mw := middleware.ForkWorkerMiddleware{}
	worker.Use(mw.WorkerAuthMiddleware())
	RegisterForkWorkerRoutes(public, storage, worker, &ForkWorkerHandler{})
	return r
}

func TestWorkerTokenRoutesAreReadOnlyExceptMlData(t *testing.T) {
	var got []string
	for _, route := range forkWorkerTestRouter().Routes() {
		if strings.HasPrefix(route.Path, "/fork/worker-api") {
			got = append(got, route.Method+" "+route.Path)
		}
	}
	sort.Strings(got)
	if strings.Join(got, "\n") != strings.Join(wantWorkerRoutes, "\n") {
		t.Fatalf("worker routes changed:\n got: %q\nwant: %q", got, wantWorkerRoutes)
	}
	for _, route := range got {
		if strings.HasPrefix(route, "DELETE ") || strings.HasPrefix(route, "PATCH ") {
			t.Errorf("worker token must not delete or patch: %s", route)
		}
	}
}

func TestWorkerAPIRejectsMissingToken(t *testing.T) {
	r := forkWorkerTestRouter()
	for _, route := range wantWorkerRoutes {
		method, path, _ := strings.Cut(route, " ")
		path = strings.ReplaceAll(path, ":fileID", "1")
		w := httptest.NewRecorder()
		r.ServeHTTP(w, httptest.NewRequest(method, path, nil))
		if w.Code != http.StatusUnauthorized {
			t.Errorf("%s without token: status %d, want 401", route, w.Code)
		}
	}
}

func TestWorkerTokenCannotReachOwnerRoutes(t *testing.T) {
	r := forkWorkerTestRouter()
	for _, route := range []string{
		"POST /fork/worker/token", "DELETE /fork/worker/token", "POST /fork/worker/inbox",
		"GET /fork/worker/inbox", "GET /fork/worker/status",
	} {
		method, path, _ := strings.Cut(route, " ")
		req := httptest.NewRequest(method, path, nil)
		req.Header.Set(forkworker.TokenHeader, "worker-token")
		w := httptest.NewRecorder()
		r.ServeHTTP(w, req)
		if w.Code != http.StatusUnauthorized {
			t.Errorf("%s with only a worker token: status %d, want 401", route, w.Code)
		}
	}
}
