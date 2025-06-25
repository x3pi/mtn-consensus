package storage

import (
	"encoding/hex"
	"errors"
	fmt "fmt"
	"sort"
	"sync"

	"github.com/ethereum/go-ethereum/common"
)

type MemoryDB struct {
	db map[[32]byte][]byte
	sync.RWMutex
}

type MemoryDbIterator struct {
	memoryDB *MemoryDB
	keys     []string
	idx      int
	sync.RWMutex
}

func NewMemoryDbIterator(memoryDB *MemoryDB) *MemoryDbIterator {
	var keys []string
	memoryDB.RLock()
	for key, _ := range memoryDB.db {
		keys = append(keys, hex.EncodeToString(key[:]))
	}
	memoryDB.RUnlock()
	sort.Strings(keys)
	return &MemoryDbIterator{
		memoryDB: memoryDB,
		keys:     keys,
		idx:      0,
	}
}

func (mdb *MemoryDB) GetIterator() IIterator {
	mdb.RLock()
	defer mdb.RUnlock()
	return NewMemoryDbIterator(mdb)
}

func (mdb *MemoryDbIterator) Next() bool {
	mdb.Lock()
	defer mdb.Unlock()
	if len(mdb.keys) == mdb.idx {
		return false
	}
	mdb.idx += 1
	return true
}

func (mdb *MemoryDbIterator) Key() []byte {
	mdb.RLock()
	defer mdb.RUnlock()
	return common.FromHex(mdb.keys[mdb.idx-1])
}

func (mdb *MemoryDbIterator) Value() []byte {
	mdb.RLock()
	mdb.memoryDB.RLock()
	defer mdb.RUnlock()
	defer mdb.memoryDB.RUnlock()
	bytes := common.FromHex(mdb.keys[mdb.idx-1])
	var bytes32 [32]byte
	copy(bytes32[:], bytes)
	return mdb.memoryDB.db[bytes32]
}

func (mdb *MemoryDbIterator) Release() {
	//ToDo:
}
func (mdb *MemoryDbIterator) Error() error {
	// ToDo:
	return nil
}

func NewMemoryDb() *MemoryDB {
	return &MemoryDB{
		db: make(map[[32]byte][]byte),
	}
}

func (kv *MemoryDB) Get(key []byte) ([]byte, error) {
	kv.RLock()
	defer kv.RUnlock()
	var bytes32 [32]byte
	copy(bytes32[:], key)
	if v, ok := kv.db[bytes32]; ok {
		return v, nil
	} else {
		return nil, errors.New(fmt.Sprintf("[MemKV] key not found: %s", hex.EncodeToString(key)))
	}
}

func (kv *MemoryDB) Put(key, value []byte) error {
	kv.Lock()
	defer kv.Unlock()
	var bytes32 [32]byte
	copy(bytes32[:], key)
	kv.db[bytes32] = value
	return nil
}

func (kv *MemoryDB) Has(key []byte) bool {
	kv.RLock()
	defer kv.RUnlock()
	var bytes32 [32]byte
	copy(bytes32[:], key)
	_, ok := kv.db[bytes32]
	return ok
}

func (kv *MemoryDB) Delete(key []byte) error {
	kv.Lock()
	defer kv.Unlock()
	var bytes32 [32]byte
	copy(bytes32[:], key)
	if _, ok := kv.db[bytes32]; ok {
		delete(kv.db, bytes32)
	} else {
		return errors.New(fmt.Sprintf("[MemKV] key not found: %s", hex.EncodeToString(key)))
	}
	return nil
}

func (kv *MemoryDB) BatchPut(kvs [][2][]byte) error {
	for i := range kvs {
		kv.Put(kvs[i][0], kvs[i][1])
	}
	return nil
}

func (kv *MemoryDB) Close() error {
	return nil
}

func (kv *MemoryDB) Open() error {
	return nil
}

func (kv *MemoryDB) Compact() error {
	return nil
}

func (kv *MemoryDB) Size() int {
	return len(kv.db)
}

func (kv *MemoryDB) GetSnapShot() SnapShot {
	newMDB := NewMemoryDb()
	iter := kv.GetIterator()
	for iter.Next() {
		// need to copy because iter done save value in next call Next
		cValue := make([]byte, len(iter.Value()))
		var cKey [32]byte
		copy(cKey[:], iter.Key())
		copy(cValue[:], iter.Value())
		newMDB.db[cKey] = cValue
	}
	return newMDB
}

func (kv *MemoryDB) Release() {}
