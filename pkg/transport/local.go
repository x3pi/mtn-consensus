// file: pkg/transport/local.go
package transport

import "github.com/meta-node-blockchain/meta-node/pkg/alea"

// LocalTransport là một triển khai mạng cục bộ để mô phỏng.
type LocalTransport struct {
	nodeID     int
	networkBus chan<- alea.Message
	nodeInbox  <-chan alea.Message
}

func NewLocalTransport(nodeID int, bus chan<- alea.Message, inbox <-chan alea.Message) *LocalTransport {
	return &LocalTransport{
		nodeID:     nodeID,
		networkBus: bus,
		nodeInbox:  inbox,
	}
}

func (lt *LocalTransport) ID() int {
	return lt.nodeID
}

func (lt *LocalTransport) Broadcast(payload interface{}) {
	msg := alea.Message{
		SenderID: lt.nodeID,
		Payload:  payload,
	}
	lt.networkBus <- msg
}

func (lt *LocalTransport) Events() <-chan alea.Message {
	return lt.nodeInbox
}
