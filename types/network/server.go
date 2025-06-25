package network

import (
	"context"

	"github.com/meta-node-blockchain/meta-node/pkg/bls"
)

type SocketServer interface {
	Listen(string) error
	Stop()

	OnConnect(Connection)
	OnDisconnect(Connection)

	SetKeyPair(*bls.KeyPair)

	HandleConnection(Connection) error

	AddOnConnectedCallBack(callBack func(Connection))
	AddOnDisconnectedCallBack(callBack func(Connection))
	SetContext(ctx context.Context, cancelFunc context.CancelFunc)
	StopAndRetryConnectToParent(conn Connection)
	RetryConnectToParent(conn Connection)
}
