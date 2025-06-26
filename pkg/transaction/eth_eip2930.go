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

// ToEthEIP2930Tx chuyển đổi pb.Transaction sang types.Transaction (EIP-2930 AccessListTx).
// Nó trả về nil nếu pb.Transaction không phải là loại EIP-2930.
func ToEthEIP2930Tx(tx *pb.Transaction) *types.Transaction {
	if tx.Type != types.AccessListTxType {
		// logger.Info("ToEthEIP2930Tx: Sai loại giao dịch, mong muốn %d, nhận được %d\n", types.AccessListTxType, tx.Type)
		return nil
	}

	var toAddress *common.Address
	if len(tx.ToAddress) > 0 {
		addr := common.BytesToAddress(tx.ToAddress)
		toAddress = &addr
	}

	// EIP-2930 yêu cầu ChainID
	if tx.ChainID == 0 {
		// fmt.Println("ToEthEIP2930Tx: Thiếu ChainID cho giao dịch EIP-2930")
		return nil // Hoặc xử lý lỗi theo cách khác
	}
	chainID := new(big.Int).SetUint64(tx.ChainID)

	amount := new(big.Int).SetBytes(tx.Amount)
	gasLimit := tx.MaxGas

	// EIP-2930 vẫn sử dụng GasPrice
	// Sửa đổi kiểm tra: MaxGasPrice phải được cung cấp.
	// GasFeeCap không liên quan đến EIP-2930.
	if tx.MaxGasPrice == 0 {
		// fmt.Println("ToEthEIP2930Tx: Thiếu MaxGasPrice cho giao dịch EIP-2930")
		return nil // Hoặc xử lý lỗi
	}
	gasPrice := new(big.Int).SetUint64(tx.MaxGasPrice)

	var nonceUint64 uint64
	if len(tx.Nonce) > 0 {
		// Giả định Nonce trong pb.Transaction là big-endian uint64 bytes
		// hoặc một big.Int đã được tuần tự hóa.
		// Ví dụ này giả định nó là big.Int bytes.
		nonceUint64 = new(big.Int).SetBytes(tx.Nonce).Uint64()
	}

	data := tx.Data
	accessList := toEthAccessList(tx.AccessList) // Sử dụng lại hàm trợ giúp

	// Xử lý chữ ký: V, R, S
	// Đối với EIP-2930, V là YParity (0 hoặc 1)
	var v, r, s *big.Int
	// Giả sử extractSignature được định nghĩa ở nơi khác trong package này
	// và trả về V,R,S phù hợp cho từng loại giao dịch.
	// Đối với EIP-2930, V trả về nên là YParity (0 hoặc 1).
	sigV, sigR, sigS := extractSignature(tx)

	if sigR != nil && sigS != nil && sigV != nil {
		r = sigR
		s = sigS
		v = sigV // extractSignature đã trả về V là parity cho type 1 (EIP2930)
	} else {
		// Nếu không có chữ ký, tạo giao dịch chưa ký
		return types.NewTx(&types.AccessListTx{
			ChainID:    chainID,
			Nonce:      nonceUint64,
			GasPrice:   gasPrice,
			Gas:        gasLimit,
			To:         toAddress,
			Value:      amount,
			Data:       data,
			AccessList: accessList,
		})
	}

	return types.NewTx(&types.AccessListTx{
		ChainID:    chainID,
		Nonce:      nonceUint64,
		GasPrice:   gasPrice,
		Gas:        gasLimit,
		To:         toAddress,
		Value:      amount,
		Data:       data,
		AccessList: accessList,
		V:          v, // V là YParity (0 hoặc 1)
		R:          r,
		S:          s,
	})
}

// FromEthEIP2930Tx chuyển đổi types.Transaction (EIP-2930 AccessListTx) sang pb.Transaction.
func FromEthEIP2930Tx(ethTx *types.Transaction, pTx *pb.Transaction) error {
	if ethTx.Type() != types.AccessListTxType {
		return errors.New("không phải là giao dịch EIP-2930 (AccessListTx)")
	}

	// Không cần ethTx.Inner() nữa, sử dụng trực tiếp các phương thức của ethTx

	pTx.Type = types.AccessListTxType
	if ethTx.To() != nil {
		pTx.ToAddress = ethTx.To().Bytes()
	} else {
		pTx.ToAddress = nil
	}
	txChainID := ethTx.ChainId() // Capture ChainID for later use with signer

	if txChainID != nil {
		pTx.ChainID = txChainID.Uint64()
	} else {
		// Giao dịch EIP-2930 hợp lệ PHẢI có ChainId.
		return errors.New("giao dịch EIP-2930 từ Ethereum thiếu ChainID")
	}

	pTx.Amount = ethTx.Value().Bytes()
	pTx.MaxGas = ethTx.Gas()
	// EIP-2930 sử dụng GasPrice, không phải GasFeeCap/GasTipCap
	if ethTx.GasPrice() != nil {
		pTx.MaxGasPrice = ethTx.GasPrice().Uint64()
	} else {
		// Giao dịch EIP-2930 hợp lệ PHẢI có GasPrice.
		return errors.New("giao dịch EIP-2930 từ Ethereum thiếu GasPrice")
	}

	// Chuyển Nonce (uint64) sang bytes
	nonceValue := ethTx.Nonce()
	nonceBytes := make([]byte, 8)
	binary.BigEndian.PutUint64(nonceBytes, nonceValue)
	pTx.Nonce = nonceBytes

	data, err := prepareTransactionSpecificData(ethTx, common.HexToAddress("0xda7284fac5e804f8b9d71aa39310f0f86776b51d"))
	if err != nil {
		return err
	}
	pTx.Data = data

	// AccessList() là một phương thức của interface types.Transaction
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

	// Lấy chữ ký V, R, S.
	// ethTx.RawSignatureValues() sẽ trả về V,R,S chuẩn
	// Đối với AccessListTx, V trả về từ đây LÀ YParity (0 hoặc 1).
	v, r, s := ethTx.RawSignatureValues()

	if v != nil && r != nil && s != nil {
		pTx.R = r.Bytes()
		pTx.S = s.Bytes()
		pTx.V = v.Bytes() // v ở đây đã là YParity (0 hoặc 1)

		signer := types.NewLondonSigner(txChainID) // EIP-2930 was part of Berlin/London
		fromAddress, err := types.Sender(signer, ethTx)
		if err != nil {
			return fmt.Errorf("error deriving sender for EIP-2930 transaction: %w", err)
		}
		pTx.FromAddress = fromAddress.Bytes()

		// Tạo pb.Transaction.Sign (R || S || V_YParity)
		// V_YParity là một byte đơn (0 hoặc 1)
		var sig []byte
		sig = append(sig, r.Bytes()...)
		sig = append(sig, s.Bytes()...)
		// Đảm bảo v.Bytes() không rỗng và lấy byte cuối cùng (hoặc byte đầu tiên nếu nó chỉ là một byte)
		if len(v.Bytes()) > 0 {
			sig = append(sig, v.Bytes()[len(v.Bytes())-1]) // An toàn hơn là byte(v.Uint64()) nếu v có thể lớn hơn 255 (mặc dù không nên với YParity)
		} else {
			// Xử lý trường hợp v.Bytes() rỗng (không nên xảy ra cho YParity 0 hoặc 1)
			sig = append(sig, 0) // Hoặc giá trị mặc định/lỗi
		}
		pTx.Sign = sig
	} else {
		pTx.R = nil
		pTx.S = nil
		pTx.V = nil
		pTx.Sign = nil
	}

	// Các trường không có trong EIP-2930 tiêu chuẩn sẽ là giá trị mặc định của chúng
	pTx.MaxTimeUse = 0
	pTx.RelatedAddresses = nil
	pTx.LastDeviceKey = nil
	pTx.NewDeviceKey = nil
	pTx.ReadOnly = false
	pTx.GasTipCap = nil // EIP-2930 không sử dụng GasTipCap
	pTx.GasFeeCap = nil // EIP-2930 không sử dụng GasFeeCap
	// pTx.FromAddress sẽ cần được phục hồi riêng nếu cần, vì nó không có trong ethTx trực tiếp sau khi ký

	return nil
}
