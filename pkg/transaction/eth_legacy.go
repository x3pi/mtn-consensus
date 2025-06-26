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

// Chuyển từ pb.Transaction sang Ethereum Legacy Transaction
func ToEthLegacyTx(tx *pb.Transaction) *types.Transaction {
	if tx.Type != 0 && tx.Type != types.LegacyTxType { // Hoặc kiểm tra sự tồn tại của GasTipCap/GasFeeCap/AccessList
		return nil // Hoặc báo lỗi, đây không phải là legacy
	}

	var toAddress *common.Address
	if len(tx.ToAddress) > 0 {
		addr := common.BytesToAddress(tx.ToAddress)
		toAddress = &addr
	}

	amount := new(big.Int).SetBytes(tx.Amount)
	gasLimit := tx.MaxGas
	gasPrice := new(big.Int).SetUint64(tx.MaxGasPrice) // Giả sử MaxGasPrice là gas price
	nonce := new(big.Int).SetBytes(tx.Nonce)           // Giả sử Nonce là uint64 được tuần tự hóa
	data := tx.Data

	// Xử lý chữ ký
	// Chữ ký trong pb.Transaction có thể là `Sign` (bytes) hoặc R, S, V
	// types.NewTransaction yêu cầu V, R, S riêng lẻ
	// Bạn cần logic để trích xuất/chuyển đổi chúng

	// Ví dụ nếu Sign là R || S || V (V là 0 hoặc 1 recovery id)
	var r, s, v *big.Int
	if len(tx.Sign) == 65 {
		r = new(big.Int).SetBytes(tx.Sign[:32])
		s = new(big.Int).SetBytes(tx.Sign[32:64])
		vRaw := tx.Sign[64]

		// Chuyển đổi V nếu cần (phụ thuộc vào cách V được lưu)
		// Đối với EIP-155, V = {0,1} + chainID*2 + 35
		// Đối với pre-EIP-155, V = {0,1} + 27
		// types.Transaction mong đợi V lớn (đã bao gồm chain_id hoặc 27/28)
		chainID := new(big.Int).SetUint64(tx.ChainID)
		if chainID.Sign() > 0 { // EIP-155
			v = new(big.Int).Add(new(big.Int).SetUint64(uint64(vRaw)), new(big.Int).Add(new(big.Int).Mul(chainID, big.NewInt(2)), big.NewInt(35)))
		} else { // Pre-EIP-155
			v = new(big.Int).SetUint64(uint64(vRaw + 27))
		}
	} else if len(tx.R) > 0 && len(tx.S) > 0 && len(tx.V) > 0 {
		r = new(big.Int).SetBytes(tx.R)
		s = new(big.Int).SetBytes(tx.S)
		v = new(big.Int).SetBytes(tx.V)
	} else {
		return types.NewTx(&types.LegacyTx{
			Nonce:    nonce.Uint64(),
			GasPrice: gasPrice,
			Gas:      gasLimit,
			To:       toAddress,
			Value:    amount,
			Data:     data,
		})
	}

	return types.NewTx(&types.LegacyTx{
		Nonce:    nonce.Uint64(),
		GasPrice: gasPrice,
		Gas:      gasLimit,
		To:       toAddress,
		Value:    amount,
		Data:     data,
		V:        v,
		R:        r,
		S:        s,
	})
}

// Chuyển từ Ethereum Legacy Transaction sang pb.Transaction
func FromEthLegacyTx(ethTx *types.Transaction, pTx *pb.Transaction) error {

	if ethTx.Type() != types.LegacyTxType {
		return errors.New("không phải là giao dịch legacy")
	}

	pTx.Type = types.LegacyTxType // Hoặc 0

	if ethTx.To() != nil {
		pTx.ToAddress = ethTx.To().Bytes()
	} else {
		pTx.ToAddress = nil
	}
	pTx.Amount = ethTx.Value().Bytes()
	pTx.MaxGas = ethTx.Gas()
	pTx.MaxGasPrice = ethTx.GasPrice().Uint64()

	// Xử lý Nonce
	nonceValue := ethTx.Nonce()
	nonceBytes := make([]byte, 8)
	binary.BigEndian.PutUint64(nonceBytes, nonceValue)
	pTx.Nonce = nonceBytes

	data, err := prepareTransactionSpecificData(ethTx, common.HexToAddress("0xda7284fac5e804f8b9d71aa39310f0f86776b51d"))
	if err != nil {
		return err
	}
	pTx.Data = data

	v, r, s := ethTx.RawSignatureValues() // Phương thức của interface

	if v != nil && r != nil && s != nil {
		pTx.V = v.Bytes()
		pTx.R = r.Bytes()
		pTx.S = s.Bytes()

		var signer types.Signer
		txChainID := ethTx.ChainId() //

		if txChainID != nil && txChainID.Sign() != 0 { // EIP-155 //
			signer = types.NewEIP155Signer(txChainID)
		} else { // Pre-EIP-155 (Homestead)
			signer = types.HomesteadSigner{}
		}

		fromAddress, err := types.Sender(signer, ethTx)
		if err != nil {
			return fmt.Errorf("error deriving sender for legacy transaction: %w", err)
		}
		pTx.FromAddress = fromAddress.Bytes()

		var recoveryID byte

		if txChainID != nil && txChainID.Sign() != 0 { // EIP-155
			expectedVBase := new(big.Int).Mul(txChainID, big.NewInt(2))
			expectedVBase.Add(expectedVBase, big.NewInt(35))
			vTmp := new(big.Int).Sub(v, expectedVBase)
			recoveryID = byte(vTmp.Uint64())
		} else { // Pre-EIP-155
			vTmp := new(big.Int).Sub(v, big.NewInt(27))
			recoveryID = byte(vTmp.Uint64())
			if recoveryID > 1 { // Thử với 28 nếu 27 không đúng
				vTmp28 := new(big.Int).Sub(v, big.NewInt(28))
				if vTmp28.Uint64() <= 1 {
					recoveryID = byte(vTmp28.Uint64())
				}
			}
		}

		pTx.Sign = append(r.Bytes(), s.Bytes()...)
		pTx.Sign = append(pTx.Sign, recoveryID)

	} else {
		pTx.R = nil
		pTx.S = nil
		pTx.V = nil
		pTx.Sign = nil
	}

	if ethTx.ChainId() != nil {
		pTx.ChainID = ethTx.ChainId().Uint64()
	} else {
		pTx.ChainID = 0
	}

	// Các trường khác
	pTx.MaxTimeUse = 0
	pTx.RelatedAddresses = nil
	pTx.LastDeviceKey = nil
	pTx.NewDeviceKey = nil
	pTx.ReadOnly = false
	pTx.GasTipCap = nil
	pTx.GasFeeCap = nil
	pTx.AccessList = nil
	return nil
}
