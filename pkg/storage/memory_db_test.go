package storage

import (
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/assert"
)

var (
	db   = NewMemoryDb()
	data = common.FromHex("f1f2f3f4")
)

func TestPutMemoryDB(t *testing.T) {
	err := db.Put(common.FromHex("f1"), data)
	assert.Nil(t, err)
}

func TestGetMemoryDB(t *testing.T) {
	db.Put(common.FromHex("f1"), data)

	val, err := db.Get(common.FromHex("f1"))
	assert.Equal(t, val, data)
	assert.Nil(t, err)

	_, err = db.Get(common.FromHex("f2"))
	assert.NotNil(t, err)
}

func TestHasMemoryDB(t *testing.T) {
	db.Put(common.FromHex("f1"), data)

	isExist := db.Has(common.FromHex("f1"))
	assert.Equal(t, isExist, true)

	isExist = db.Has(common.FromHex("f2"))
	assert.Equal(t, isExist, false)
}

func TestDeleteMemoryDB(t *testing.T) {
	db.Put(common.FromHex("f1"), data)
	err := db.Delete(common.FromHex("f1"))
	assert.Nil(t, err)

	err = db.Delete(common.FromHex("f1"))
	assert.NotNil(t, err)
}

func TestBatchPutMemoryDB(t *testing.T) {
	a := [][2][]byte{
		{
			common.FromHex("f1"),
			common.FromHex("f3"),
		},
	}

	err := db.BatchPut(a)
	assert.Nil(t, err)
}
