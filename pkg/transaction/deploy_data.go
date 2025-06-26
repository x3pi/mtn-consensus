package transaction

import (
	"github.com/ethereum/go-ethereum/common"
	"google.golang.org/protobuf/proto"

	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/types"
)

type DeployData struct {
	proto *pb.DeployData
}

func NewDeployData(
	code []byte,
	storageAddress common.Address,
) types.DeployData {
	return &DeployData{
		proto: &pb.DeployData{
			Code:           code,
			StorageAddress: storageAddress.Bytes(),
		},
	}
}

func (dd *DeployData) Unmarshal(b []byte) error {
	ddPb := &pb.DeployData{}
	err := proto.Unmarshal(b, ddPb)
	if err != nil {
		return err
	}
	dd.proto = ddPb
	return nil
}

func (dd *DeployData) Marshal() ([]byte, error) {
	return proto.Marshal(dd.proto)
}

// geter
func (dd *DeployData) Code() []byte {
	return dd.proto.Code
}

func (dd *DeployData) StorageAddress() common.Address {
	return common.BytesToAddress(dd.proto.StorageAddress)
}
