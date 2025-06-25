package storage

import (
	"fmt"
	"sync"
	"time"

	"github.com/dgraph-io/badger/v4"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
)

type BadgerV4DBManager struct {
	instances    map[string]*BadgerV4DB
	mu           sync.RWMutex
	maxOpenFiles int
	idleTimeout  time.Duration
}

type BadgerV4DB struct {
	db         *badger.DB
	path       string
	closeChan  chan bool
	lastActive time.Time
	mu         sync.Mutex
}

func NewBadgerV4DBManager(maxOpenFiles int, idleTimeout time.Duration) *BadgerV4DBManager {
	mgr := &BadgerV4DBManager{
		instances:    make(map[string]*BadgerV4DB),
		maxOpenFiles: maxOpenFiles,
		idleTimeout:  idleTimeout,
	}
	go mgr.periodicCleanup(mgr.idleTimeout)
	return mgr
}

func (mgr *BadgerV4DBManager) periodicCleanup(duration time.Duration) {
	for {
		mgr.cleanupInstances(duration)
		time.Sleep(duration)
	}
}

func (mgr *BadgerV4DBManager) cleanupInstances(duration time.Duration) {
	mgr.mu.Lock()
	defer mgr.mu.Unlock()

	for path, db := range mgr.instances {
		if time.Since(db.lastActive) >= duration {
			logger.Info("BadgerV4DB cleaned up path: ", path)
			if err := db.Close(); err != nil {
				logger.Error("Failed to close BadgerV4DB at path %s during cleanup: %v", path, err)
			}
			delete(mgr.instances, path)
		}
	}
	logger.Info("BadgerV4DB instances cleaned up.")
}

func (mgr *BadgerV4DBManager) GetOrCreate(path string) (*BadgerV4DB, error) {
	mgr.mu.Lock()
	defer mgr.mu.Unlock()

	if path == "" {
		return nil, fmt.Errorf("invalid path: path is empty")
	}

	if db, exists := mgr.instances[path]; exists {
		return db, nil
	}

	opts := badger.DefaultOptions(path).WithLogger(nil)
	db, err := badger.Open(opts)
	if err != nil {
		return nil, fmt.Errorf("failed to open BadgerV4DB: %v", err)
	}

	BadgerV4DB := &BadgerV4DB{
		db:         db,
		path:       path,
		closeChan:  make(chan bool),
		lastActive: time.Now(),
	}

	go BadgerV4DB.manageIdle(mgr.idleTimeout)
	mgr.instances[path] = BadgerV4DB
	return BadgerV4DB, nil
}

func (db *BadgerV4DB) manageIdle(idleTimeout time.Duration) {
	for {
		select {
		case <-time.After(idleTimeout):
			db.mu.Lock()
			idleDuration := time.Since(db.lastActive)
			db.mu.Unlock()

			if idleDuration >= idleTimeout {
				logger.Info("Closing idle BadgerV4DB connection at path: %s", db.path)
				if err := db.Close(); err != nil {
					logger.Error("Failed to close idle BadgerV4DB: %v", err)
				}
				return
			}
		case <-db.closeChan:
			return
		}
	}
}

func (db *BadgerV4DB) updateLastActive() {
	db.mu.Lock()
	defer db.mu.Unlock()
	db.lastActive = time.Now()
}

func (db *BadgerV4DB) Get(key []byte) ([]byte, error) {
	db.updateLastActive()

	var value []byte
	err := db.db.View(func(txn *badger.Txn) error {
		item, err := txn.Get(key)
		if err != nil {
			return err
		}
		value, err = item.ValueCopy(nil)
		return err
	})
	return value, err
}

func (db *BadgerV4DB) Put(key, value []byte) error {
	db.updateLastActive()

	return db.db.Update(func(txn *badger.Txn) error {
		return txn.Set(key, value)
	})
}

func (db *BadgerV4DB) Delete(key []byte) error {
	db.updateLastActive()

	return db.db.Update(func(txn *badger.Txn) error {
		return txn.Delete(key)
	})
}

func (db *BadgerV4DB) Close() error {
	db.mu.Lock()
	defer db.mu.Unlock()

	db.closeChan <- true
	return db.db.Close()
}

func (db *BadgerV4DB) Compact() error {
	err := db.db.RunValueLogGC(0.5) // GC dữ liệu thừa với ngưỡng 50%
	if err != nil && err != badger.ErrNoRewrite {
		return fmt.Errorf("GC failed: %v", err)
	}
	return nil
}

func (db *BadgerV4DB) BatchPut(kvs [][2][]byte) error {
	db.updateLastActive()

	// Sử dụng WriteBatch để tối ưu ghi hàng loạt
	return db.db.Update(func(txn *badger.Txn) error {
		for _, kv := range kvs {
			if len(kv) != 2 {
				return fmt.Errorf("invalid key-value pair: %v", kv)
			}
			key := kv[0]
			value := kv[1]
			if err := txn.Set(key, value); err != nil {
				return fmt.Errorf("failed to set key: %v, error: %v", key, err)
			}
		}
		return nil
	})
}

func (db *BadgerV4DB) Has(key []byte) bool {
	db.updateLastActive()

	err := db.db.View(func(txn *badger.Txn) error {
		_, err := txn.Get(key)
		return err
	})

	return err == nil
}

func (db *BadgerV4DB) Open() error {
	db.mu.Lock()
	defer db.mu.Unlock()

	if db.db == nil {
		opts := badger.DefaultOptions(db.path).WithLogger(nil)
		newDb, err := badger.Open(opts)
		if err != nil {
			return fmt.Errorf("failed to reopen BadgerV4DB: %v", err)
		}
		db.db = newDb
	}
	return nil
}

type BadgerIterator struct {
	iterator *badger.Iterator
	txn      *badger.Txn
	err      error
}

func (it *BadgerIterator) Next() bool {
	if it.iterator.Valid() {
		it.iterator.Next()
	}
	return it.iterator.Valid()
}

func (it *BadgerIterator) Key() []byte {
	if it.iterator.Valid() {
		return it.iterator.Item().KeyCopy(nil)
	}
	return nil
}

func (it *BadgerIterator) Value() []byte {
	if it.iterator.Valid() {
		value, err := it.iterator.Item().ValueCopy(nil)
		if err != nil {
			it.err = err
			return nil
		}
		return value
	}
	return nil
}

func (it *BadgerIterator) Release() {
	it.iterator.Close()
	it.txn.Discard()
}

func (it *BadgerIterator) Error() error {
	return it.err
}

func (db *BadgerV4DB) GetIterator() IIterator {
	db.updateLastActive()

	txn := db.db.NewTransaction(false) // Tạo transaction readonly
	iterator := txn.NewIterator(badger.DefaultIteratorOptions)

	return &BadgerIterator{
		iterator: iterator,
		txn:      txn,
	}
}

type BadgerSnapShot struct {
	txn *badger.Txn // Transaction trong chế độ View
}

func (s *BadgerSnapShot) GetIterator() IIterator {
	opts := badger.IteratorOptions{
		PrefetchSize: 1024, // Ví dụ: PrefetchSize
		Reverse:      true, // Ví dụ: Reverse
		// ... các tùy chọn khác ...
	}
	// Trả về Iterator từ transaction trong chế độ View
	return &BadgerIterator{
		iterator: s.txn.NewIterator(opts),
	}
}

func (s *BadgerSnapShot) Release() {
	// Giải phóng transaction
	s.txn.Discard()
}

func (db *BadgerV4DB) GetSnapShot() SnapShot {
	// Lấy snapshot từ BadgerDB và tạo đối tượng BadgerSnapShot
	txn := db.db.NewTransaction(false) // Sử dụng NewTransaction(false) thay cho NewSnapshot()
	return &BadgerSnapShot{txn: txn}   // Sửa đổi để truyền txn vào BadgerSnapShot
}
