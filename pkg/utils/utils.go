package utils

import (
	"encoding/binary"
	"fmt"
	"math/big"
	"strconv"
	"strings"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/crypto"
)

// Uint64ToBytes converts a uint64 to a byte array.
func Uint64ToBytes(value uint64) []byte {
	bytes := make([]byte, 8)
	binary.BigEndian.PutUint64(bytes, value)
	return bytes
}

// BytesToUint64 converts a byte array to a uint64.
func BytesToUint64(bytes []byte) (uint64, error) {
	if len(bytes) != 8 {
		return 0, fmt.Errorf("byte array must be 8 bytes long")
	}
	return binary.BigEndian.Uint64(bytes), nil
}

// Uint64ToBigInt converts a uint64 to a big.Int.
func Uint64ToBigInt(value uint64) *big.Int {
	return new(big.Int).SetUint64(value)
}

// BigIntToUint64 converts a big.Int to a uint64.
func BigIntToUint64(value *big.Int) (uint64, error) {
	if value == nil {
		return 0, fmt.Errorf("big.Int không được phép là nil")
	}
	if value.IsUint64() {
		return value.Uint64(), nil
	}
	return 0, fmt.Errorf("big.Int quá lớn để chuyển đổi thành uint64")
}

func HexutilBigToUint64(n *hexutil.Big) (uint64, error) {
	if n == nil {
		return 0, fmt.Errorf("input hexutil.Big is nil")
	}
	bigInt := n.ToInt()
	uint64Value := bigInt.Uint64()
	return uint64Value, nil
}

func ParseBlockNumber(blockNumberStr string) (uint64, error) {
	// Kiểm tra xem blockNumberStr đã có tiền tố "0x" chưa
	blockNumberStr = strings.TrimPrefix(blockNumberStr, "0x")

	blockNumber, err := strconv.ParseUint(blockNumberStr, 16, 64)
	if err != nil {
		return 0, fmt.Errorf("lỗi chuyển đổi block number: %w", err)
	}

	return blockNumber, nil
}

// GetFunctionSelector tính function selector (4 byte đầu) từ tên hàm Solidity
func GetFunctionSelector(methodSignature string) []byte {
	hash := crypto.Keccak256([]byte(methodSignature))
	return hash[:4] // Lấy 4 byte đầu
}

// GetFunctionSelector tính function selector (4 byte đầu) từ tên hàm Solidity
func GetAddressSelector(methodSignature string) common.Address {
	hash := crypto.Keccak256([]byte(methodSignature))
	return common.BytesToAddress(hash[:4])
}

// Hàm hỗ trợ so sánh uint64
func CompareUint64(a, b uint64) int {
	if a < b {
		return -1
	} else if a > b {
		return 1
	}
	return 0
}
