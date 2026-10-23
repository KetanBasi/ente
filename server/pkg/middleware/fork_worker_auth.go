package middleware

// fork: auth for the home AI worker's read-only token. See
// pkg/controller/forkworker.

import (
	"net/http"

	"github.com/ente/museum/ente/forkworker"
	workerCtrl "github.com/ente/museum/pkg/controller/forkworker"
	"github.com/gin-gonic/gin"
)

type ForkWorkerMiddleware struct {
	Ctrl *workerCtrl.Controller
}

func (m *ForkWorkerMiddleware) WorkerAuthMiddleware() gin.HandlerFunc {
	return func(c *gin.Context) {
		token := c.GetHeader(forkworker.TokenHeader)
		if token == "" {
			c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{"error": "worker access token missing"})
			return
		}
		authCtx, err := m.Ctrl.Authenticate(c, token)
		if err != nil {
			c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{"error": "invalid token"})
			return
		}
		c.Set(forkworker.AuthContextKey, *authCtx)
		c.Next()
	}
}
