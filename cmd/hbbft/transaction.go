package main

import (
	"encoding/binary"
	"math/rand"

	"github.com/anthdm/hbbft"
)

// Transaction là một triển khai đơn giản của giao diện hbbft.Transaction.
type Transaction struct {
	Nonce uint64
}

// Đảm bảo Transaction tuân thủ giao diện của hbbft.
var _ hbbft.Transaction = (*Transaction)(nil)

// Hash trả về một hash duy nhất cho giao dịch.
func (t *Transaction) Hash() []byte {
	buf := make([]byte, 8)
	binary.LittleEndian.PutUint64(buf, t.Nonce)
	return buf
}

// newTransaction tạo một giao dịch mới với một nonce ngẫu nhiên.
func newTransaction() *Transaction {
	return &Transaction{Nonce: rand.Uint64()}
}
