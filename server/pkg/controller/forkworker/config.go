package forkworker

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"

	"github.com/ente/museum/ente/forkworker"
	"github.com/spf13/viper"
)

// ConfigKey is the museum config key that holds the worker allowlist:
//
//	fork:
//	    worker-keys:
//	        - name: home-pc
//	          public-key: <base64 X25519 public key>
const ConfigKey = "fork.worker-keys"

// LoadKeys reads and validates the worker allowlist from museum config.
// An empty list turns the feature off.
func LoadKeys() ([]forkworker.Key, error) {
	var keys []forkworker.Key
	if err := viper.UnmarshalKey(ConfigKey, &keys); err != nil {
		return nil, fmt.Errorf("%s: %w", ConfigKey, err)
	}
	return ValidateKeys(keys)
}

// ValidateKeys checks each key is a base64 32 byte X25519 public key with a
// unique name, and fills in its fingerprint.
func ValidateKeys(keys []forkworker.Key) ([]forkworker.Key, error) {
	names := make(map[string]bool, len(keys))
	pubKeys := make(map[string]bool, len(keys))
	res := make([]forkworker.Key, 0, len(keys))
	for i, k := range keys {
		if k.Name == "" {
			return nil, fmt.Errorf("%s[%d]: name is required", ConfigKey, i)
		}
		raw, err := base64.StdEncoding.Strict().DecodeString(k.PublicKey)
		if err != nil || len(raw) != forkworker.PublicKeyBytes {
			return nil, fmt.Errorf("%s[%d] (%s): public-key must be %d bytes of standard base64", ConfigKey, i, k.Name, forkworker.PublicKeyBytes)
		}
		if names[k.Name] || pubKeys[k.PublicKey] {
			return nil, fmt.Errorf("%s[%d] (%s): duplicate name or public-key", ConfigKey, i, k.Name)
		}
		names[k.Name] = true
		pubKeys[k.PublicKey] = true
		k.Fingerprint = Fingerprint(raw)
		res = append(res, k)
	}
	return res, nil
}

// Fingerprint is the hex SHA-256 of a raw public key. Clients show it when
// the user turns the worker on, and must compute it themselves.
func Fingerprint(rawPublicKey []byte) string {
	sum := sha256.Sum256(rawPublicKey)
	return hex.EncodeToString(sum[:])
}
