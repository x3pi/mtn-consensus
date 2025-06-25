package storage

import (
	"encoding/hex"
	"errors"
	"sync"
	"time"

	"github.com/dgraph-io/badger"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	cp "github.com/otiai10/copy"
)

type BadgerDB struct {
	db     *badger.DB
	closed bool
	path   string
	mu     *sync.RWMutex
}

// badger iterator
type BadgerDBIterator struct {
	db          *badger.DB
	txn         *badger.Txn
	iterator    *badger.Iterator
	currentItem *badger.Item
}

func NewBadgerDB(path string) (*BadgerDB, error) {
	db, err := badger.Open(badger.DefaultOptions(path))
	if err != nil {
		return nil, err
	}

	// Run garbage collection every hour.
	go func() {
		for range time.Tick(time.Minute) {
			db.RunValueLogGC(0.7)
		}
	}()

	return &BadgerDB{
		db:     db,
		closed: false,
		path:   path,
		mu:     &sync.RWMutex{},
	}, nil
}

func (bdb *BadgerDB) Get(key []byte) ([]byte, error) {
	if bdb.closed {
		return nil, errors.New("database is closed")
	}

	var value []byte
	err := bdb.db.View(func(txn *badger.Txn) error {
		item, err := txn.Get(key)
		if err != nil {
			return err
		}
		return item.Value(func(val []byte) error {
			value = append([]byte{}, val...)
			return nil
		})
	})
	if err != nil && err != badger.ErrKeyNotFound {
		return nil, err
	}

	return value, nil
}

func (bdb *BadgerDB) Put(key, value []byte) error {
	bdb.mu.Lock()
	defer bdb.mu.Unlock()

	if bdb.closed {
		return errors.New("database is closed")
	}

	err := bdb.db.Update(func(txn *badger.Txn) error {
		return txn.Set(key, value)
	})

	return err
}

func (bdb *BadgerDB) Has(key []byte) bool {
	if bdb.closed {
		return false
	}

	err := bdb.db.View(func(txn *badger.Txn) error {
		_, err := txn.Get(key)
		if err != nil {
			return err
		}
		return nil
	})

	return err == nil
}

func (bdb *BadgerDB) Delete(key []byte) error {
	bdb.mu.Lock()
	defer bdb.mu.Unlock()

	if bdb.closed {
		return errors.New("database is closed")
	}

	err := bdb.db.Update(func(txn *badger.Txn) error {
		return txn.Delete(key)
	})

	return err
}

func (bdb *BadgerDB) BatchPut(kvs [][2][]byte) error {
	bdb.mu.Lock()
	defer bdb.mu.Unlock()

	if bdb.closed {
		return errors.New("database is closed")
	}

	err := bdb.db.Update(func(txn *badger.Txn) error {
		for _, kv := range kvs {
			if err := txn.Set(kv[0], kv[1]); err != nil {
				return err
			}
		}
		return nil
	})

	return err
}

func (bdb *BadgerDB) Open() error {
	if !bdb.closed {
		return nil
	}
	var err error
	bdb.db, err = badger.Open(badger.DefaultOptions(bdb.path))
	if err != nil {
		return err
	}
	bdb.closed = false
	return nil
}

func (bdb *BadgerDB) Close() error {
	if bdb.closed {
		return nil
	}

	err := bdb.db.Close()
	bdb.closed = true

	return err
}

func (bdb *BadgerDB) GetSnapShot() SnapShot {
	bdb.mu.Lock()
	defer bdb.mu.Unlock()
	snapShotDbPath := bdb.path + "snap_shot"
	err := cp.Copy(bdb.path, snapShotDbPath)
	if err != nil {
		logger.Error("Error when copy badger db to create snapshot")
		return nil
	}
	snapShot, err := NewBadgerDB(snapShotDbPath)
	if err != nil {
		logger.Error("Error when create snapshot badger db")
	}
	return snapShot
}

func (bdb *BadgerDB) GetIterator() IIterator {
	return NewBadgerDBIterator(bdb)
}

func (bdb *BadgerDB) Release() {
	bdb.db.DropAll()
	bdb.db.Close()
}

func NewBadgerDBIterator(bdb *BadgerDB) *BadgerDBIterator {
	txn := bdb.db.NewTransaction(true)
	opts := badger.DefaultIteratorOptions
	opts.PrefetchSize = 1000
	iterator := txn.NewIterator(opts)
	iterator.Rewind()
	return &BadgerDBIterator{
		db:       bdb.db,
		txn:      txn,
		iterator: iterator,
	}
}

func (bIter *BadgerDBIterator) Next() bool {
	if bIter.iterator.Valid() {
		item := bIter.iterator.Item()
		k := item.Key()
		item.Value(func(v []byte) error {
			logger.Info("key=%s, value=%s\n", hex.EncodeToString(k), hex.EncodeToString(v))
			return nil
		})
		bIter.currentItem = bIter.iterator.Item()
		bIter.iterator.Next()
		return true
	}
	// Rewind the iterator to the beginning of the database.
	return false
}

func (bIter *BadgerDBIterator) Key() []byte {
	return bIter.currentItem.Key()
}

func (bIter *BadgerDBIterator) Value() []byte {
	v, _ := bIter.currentItem.ValueCopy(nil)
	return v
}

func (bIter *BadgerDBIterator) Release() {
}

func (bIter *BadgerDBIterator) Error() error {
	return nil
}
