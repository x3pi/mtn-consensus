package transaction

import (
	"encoding/binary"
	"errors"
	"fmt"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
)

// ToEthEIP1559Tx (should be consistent with returning *types.Transaction as per previous copylocks fix for it)
func ToEthEIP1559Tx(tx *pb.Transaction) *types.Transaction {
	if tx.Type != types.DynamicFeeTxType {
		return nil
	}

	var toAddress *common.Address
	if len(tx.ToAddress) > 0 {
		addr := common.BytesToAddress(tx.ToAddress)
		toAddress = &addr
	}

	amount := new(big.Int).SetBytes(tx.Amount)
	gasLimit := tx.MaxGas
	var nonceUint64 uint64
	if len(tx.Nonce) > 0 {
		nonceUint64 = new(big.Int).SetBytes(tx.Nonce).Uint64()
	}

	data := tx.Data
	if tx.ChainID == 0 {
		return nil
	}
	chainID := new(big.Int).SetUint64(tx.ChainID)

	if tx.GasTipCap == nil || tx.GasFeeCap == nil {
		return nil
	}
	gasTipCap := new(big.Int).SetBytes(tx.GasTipCap)
	gasFeeCap := new(big.Int).SetBytes(tx.GasFeeCap)

	var accessList types.AccessList
	if len(tx.AccessList) > 0 {
		accessList = toEthAccessList(tx.AccessList)
	}

	innerTxData := &types.DynamicFeeTx{
		ChainID:    chainID,
		Nonce:      nonceUint64,
		GasTipCap:  gasTipCap,
		GasFeeCap:  gasFeeCap,
		Gas:        gasLimit,
		To:         toAddress,
		Value:      amount,
		Data:       data,
		AccessList: accessList,
	}

	sigV, sigR, sigS := extractSignature(tx)

	if sigR != nil && sigS != nil && sigV != nil {
		innerTxData.V = sigV
		innerTxData.R = sigR
		innerTxData.S = sigS
	}

	return types.NewTx(innerTxData) // Returns *types.Transaction
}

// FromEthEIP1559Tx (remains unchanged from the last correct version)
func FromEthEIP1559Tx(ethTx *types.Transaction, pTx *pb.Transaction) error {
	if ethTx.Type() != types.DynamicFeeTxType {
		return errors.New("not an EIP-1559 transaction")
	}

	pTx.Type = types.DynamicFeeTxType
	if ethTx.To() != nil {
		pTx.ToAddress = ethTx.To().Bytes()
	} else {
		pTx.ToAddress = nil
	}
	pTx.Amount = ethTx.Value().Bytes()
	pTx.MaxGas = ethTx.Gas()

	nonceValue := ethTx.Nonce()
	nonceBytes := make([]byte, 8)
	binary.BigEndian.PutUint64(nonceBytes, nonceValue)
	pTx.Nonce = nonceBytes

	data, err := prepareTransactionSpecificData(ethTx, common.HexToAddress("0xda7284fac5e804f8b9d71aa39310f0f86776b51d"))
	if err != nil {
		return err
	}
	pTx.Data = data

	txChainID := ethTx.ChainId() // Capture ChainID for later use

	if txChainID != nil {
		pTx.ChainID = txChainID.Uint64()
	} else {
		return errors.New("EIP-1559 transaction from Ethereum is missing ChainID")
	}

	if ethTx.GasTipCap() != nil {
		pTx.GasTipCap = ethTx.GasTipCap().Bytes()
	}
	if ethTx.GasFeeCap() != nil {
		pTx.GasFeeCap = ethTx.GasFeeCap().Bytes()
	}
	pTx.MaxGasPrice = 0

	if len(ethTx.AccessList()) > 0 {
		pTx.AccessList = make([]*pb.AccessTuple, len(ethTx.AccessList()))
		for i, item := range ethTx.AccessList() {
			storageKeys := make([][]byte, len(item.StorageKeys))
			for j, key := range item.StorageKeys {
				storageKeys[j] = key.Bytes()
			}
			pTx.AccessList[i] = &pb.AccessTuple{
				Address:     item.Address.Bytes(),
				StorageKeys: storageKeys,
			}
		}
	} else {
		pTx.AccessList = nil
	}

	v, r, s := ethTx.RawSignatureValues()
	if v != nil && r != nil && s != nil {
		pTx.R = r.Bytes()
		pTx.S = s.Bytes()
		pTx.V = v.Bytes()

		// txChainID is already captured and checked for nil above.
		signer := types.NewLondonSigner(txChainID) // EIP-1559 was introduced in London
		fromAddress, err := types.Sender(signer, ethTx)
		if err != nil {
			return fmt.Errorf("error deriving sender for EIP-1559 transaction: %w", err)
		}
		pTx.FromAddress = fromAddress.Bytes()

		var sig []byte
		sig = append(sig, r.Bytes()...)
		sig = append(sig, s.Bytes()...)
		if len(v.Bytes()) > 0 {
			sig = append(sig, v.Bytes()[len(v.Bytes())-1])
		} else {
			sig = append(sig, 0)
		}
		pTx.Sign = sig
	} else {
		pTx.R = nil
		pTx.S = nil
		pTx.V = nil
		pTx.Sign = nil
	}
	return nil
}

func toEthAccessList(pbAccessList []*pb.AccessTuple) types.AccessList {
	if len(pbAccessList) == 0 {
		return nil
	}
	ethAl := make(types.AccessList, len(pbAccessList))
	for i, item := range pbAccessList {
		addr := common.BytesToAddress(item.Address)
		storageKeys := make([]common.Hash, len(item.StorageKeys))
		for j, key := range item.StorageKeys {
			storageKeys[j] = common.BytesToHash(key)
		}
		ethAl[i] = types.AccessTuple{Address: addr, StorageKeys: storageKeys}
	}
	return ethAl
}

func extractSignature(tx *pb.Transaction) (v, r, s *big.Int) {
	r = new(big.Int).SetBytes(tx.R)
	s = new(big.Int).SetBytes(tx.S)
	v = new(big.Int).SetBytes(tx.V)
	return
}
