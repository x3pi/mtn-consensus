package transaction

import (
	"google.golang.org/protobuf/proto"

	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/types"
)

type ExecuteSCTransactions struct {
	transactions []types.Transaction
	blockNumber  uint64
	groupId      uint64
}

func NewExecuteSCTransactions(
	transactions []types.Transaction,
	groupId uint64,
	blockNumber uint64,
) *ExecuteSCTransactions {
	return &ExecuteSCTransactions{
		transactions: transactions,
		blockNumber:  blockNumber,
		groupId:      groupId,
	}
}

func (et *ExecuteSCTransactions) Transactions() []types.Transaction {
	return et.transactions
}

func (et *ExecuteSCTransactions) GroupId() uint64 {
	return et.groupId
}

func (et *ExecuteSCTransactions) BlockNumber() uint64 {
	return et.blockNumber
}

func (et *ExecuteSCTransactions) Marshal() ([]byte, error) {
	return proto.Marshal(et.Proto())
}

func (et *ExecuteSCTransactions) Unmarshal(data []byte) error {
	pbData := &pb.ExecuteSCTransactions{}
	err := proto.Unmarshal(data, pbData)
	if err != nil {
		return err
	}
	et.FromProto(pbData)
	return nil
}

func (et *ExecuteSCTransactions) Proto() *pb.ExecuteSCTransactions {
	transactions := make([]*pb.Transaction, len(et.transactions))
	for i, t := range et.transactions {
		transactions[i] = t.Proto().(*pb.Transaction)
	}
	return &pb.ExecuteSCTransactions{
		Transactions: transactions,
		GroupId:      et.groupId,
		BlockNumber:  et.blockNumber,
	}
}

func (et *ExecuteSCTransactions) FromProto(pbData *pb.ExecuteSCTransactions) {
	transactions := make([]types.Transaction, len(pbData.Transactions))
	for i, t := range pbData.Transactions {
		transactions[i] = &Transaction{}
		transactions[i].FromProto(t)
	}
	et.transactions = transactions
	et.groupId = pbData.GroupId
	et.blockNumber = pbData.BlockNumber
}
