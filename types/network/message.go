package network

import (
	e_common "github.com/ethereum/go-ethereum/common"
	"google.golang.org/protobuf/reflect/protoreflect"

	"github.com/meta-node-blockchain/meta-node/pkg/common"
)

type Message interface {
	Marshal() ([]byte, error)
	Unmarshal(protoStruct protoreflect.ProtoMessage) error
	String() string
	// getter
	Command() string
	Body() []byte
	ToAddress() e_common.Address
	Pubkey() common.PublicKey
	Sign() common.Sign
	ID() string
}
