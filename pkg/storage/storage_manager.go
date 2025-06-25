package storage

import (
	"errors"
	"sync"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
)

var (
	LastBlockNumberHashKey common.Hash = common.BytesToHash(crypto.Keccak256([]byte("lastBlockNumberHashKey")))
)

// StorageType sử dụng enum (iota) để định danh loại Storage
type StorageType int

const (
	STORAGE_ACCOUNT StorageType = iota
	STORAGE_BACKUP_DEVICE_KEY
	STORAGE_RECEIPTS
	STORAGE_SMART_CONTRACT
	STORAGE_CODE
	STORAGE_DATABASE_TRIE
	STORAGE_BLOCK
	STORAGE_BACKUP_DB
	STORAGE_MAPPING_DB
	STORAGE_TRANSACTION
)

// String() giúp in giá trị enum dễ đọc hơn
func (s StorageType) String() string {
	switch s {
	case STORAGE_ACCOUNT:
		return "ACCOUNT_DB"
	case STORAGE_BACKUP_DEVICE_KEY:
		return "STORAGE_BACKUP_DEVICE_KEY"
	case STORAGE_RECEIPTS:
		return "STORAGE_RECEIPTS"
	case STORAGE_SMART_CONTRACT:
		return "STORAGE_SMART_CONTRACT"
	case STORAGE_CODE:
		return "STORAGE_CODE"
	case STORAGE_DATABASE_TRIE:
		return "STORAGE_DATABASE_TRIE"
	case STORAGE_BLOCK:
		return "STORAGE_BLOCK"
	case STORAGE_BACKUP_DB:
		return "STORAGE_BACKUP_DB"
	case STORAGE_MAPPING_DB:
		return "STORAGE_MAPPING_DB"
	case STORAGE_TRANSACTION:
		return "STORAGE_TRANSACTION"
	default:
		return "UNKNOWN"
	}
}

// Danh sách hợp lệ các loại storage
var validStorageTypes = map[StorageType]bool{
	STORAGE_ACCOUNT:           true,
	STORAGE_BACKUP_DEVICE_KEY: true,
	STORAGE_RECEIPTS:          true,
	STORAGE_SMART_CONTRACT:    true,
	STORAGE_CODE:              true,
	STORAGE_DATABASE_TRIE:     true,
	STORAGE_BLOCK:             true,
	STORAGE_BACKUP_DB:         true,
	STORAGE_MAPPING_DB:        true,
	STORAGE_TRANSACTION:       true,
}

// StorageManager quản lý nhiều Storage với enum key
type StorageManager struct {
	storages  map[StorageType]Storage
	shardelDB map[StorageType]*ShardelDB

	mu sync.RWMutex
}

// Khởi tạo StorageManager
func NewStorageManager() *StorageManager {
	return &StorageManager{
		storages:  make(map[StorageType]Storage),
		shardelDB: make(map[StorageType]*ShardelDB), // Khởi tạo shardelDB
	}
}

// Thêm Storage vào manager (đảm bảo chỉ thêm 1 lần và hợp lệ)
func (sm *StorageManager) AddStorage(dbType StorageType, storage Storage) error {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	// Kiểm tra loại storage có hợp lệ không
	if _, valid := validStorageTypes[dbType]; !valid {
		return errors.New("invalid storage type")
	}

	// Kiểm tra nếu storage đã tồn tại
	if _, exists := sm.storages[dbType]; exists {
		return errors.New("storage type already exists")
	}

	sm.storages[dbType] = storage
	return nil
}

// Lấy Storage theo loại (enum key)
func (sm *StorageManager) GetStorage(dbType StorageType) Storage {
	sm.mu.RLock()
	defer sm.mu.RUnlock()
	storage, exists := sm.storages[dbType]
	if !exists {
		return nil
	}
	return storage
}

// Thêm ShardelDB vào manager (đảm bảo chỉ thêm 1 lần và hợp lệ)
func (sm *StorageManager) AddStorageShardelDB(dbType StorageType, storage *ShardelDB) error {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	// Kiểm tra loại storage có hợp lệ không
	if _, valid := validStorageTypes[dbType]; !valid {
		return errors.New("invalid storage type")
	}

	// Kiểm tra nếu storage đã tồn tại
	if _, exists := sm.shardelDB[dbType]; exists {
		return errors.New("storage type already exists")
	}

	sm.shardelDB[dbType] = storage
	return nil
}

// Lấy ShardelDB theo loại (enum key)
func (sm *StorageManager) GetStorageShardelDB(dbType StorageType) *ShardelDB {
	sm.mu.RLock()
	defer sm.mu.RUnlock()
	storage, exists := sm.shardelDB[dbType]
	if !exists {
		return nil
	}
	return storage
}

// Các hàm AddStorage riêng cho từng loại
func (sm *StorageManager) AddStorageAccount(storage Storage) error {
	return sm.AddStorage(STORAGE_ACCOUNT, storage)
}

func (sm *StorageManager) GetStorageAccount() Storage {
	return sm.GetStorage(STORAGE_ACCOUNT)
}

func (sm *StorageManager) AddStorageTransaction(storage Storage) error {
	return sm.AddStorage(STORAGE_TRANSACTION, storage)
}

func (sm *StorageManager) GetStorageTransaction() Storage {
	return sm.GetStorage(STORAGE_TRANSACTION)
}

func (sm *StorageManager) AddStorageBackupDeviceKey(storage Storage) error {
	return sm.AddStorage(STORAGE_BACKUP_DEVICE_KEY, storage)
}

func (sm *StorageManager) GetStorageBackupDeviceKey() Storage {
	return sm.GetStorage(STORAGE_BACKUP_DEVICE_KEY)
}

func (sm *StorageManager) AddStorageReceipt(storage Storage) error {
	return sm.AddStorage(STORAGE_RECEIPTS, storage)
}

func (sm *StorageManager) GetStorageReceipt() Storage {
	return sm.GetStorage(STORAGE_RECEIPTS)
}

func (sm *StorageManager) AddStorageSmartContract(storage Storage) error {
	return sm.AddStorage(STORAGE_SMART_CONTRACT, storage)
}

func (sm *StorageManager) GetStorageSmartContract() Storage {
	return sm.GetStorage(STORAGE_SMART_CONTRACT)
}

func (sm *StorageManager) AddStorageCode(storage Storage) error {
	return sm.AddStorage(STORAGE_CODE, storage)
}

func (sm *StorageManager) GetStorageCode() Storage {
	return sm.GetStorage(STORAGE_CODE)
}

func (sm *StorageManager) AddStorageDatabaseTrie(storage *ShardelDB) error {
	return sm.AddStorageShardelDB(STORAGE_DATABASE_TRIE, storage)
}

func (sm *StorageManager) GetStorageDatabaseTrie() *ShardelDB {
	return sm.GetStorageShardelDB(STORAGE_DATABASE_TRIE)

}

func (sm *StorageManager) AddStorageBlock(storage *ShardelDB) error {
	return sm.AddStorageShardelDB(STORAGE_BLOCK, storage)
}

func (sm *StorageManager) GetStorageBlock() *ShardelDB {
	return sm.GetStorageShardelDB(STORAGE_BLOCK)
}

func (sm *StorageManager) AddStorageBackupDb(storage Storage) error {
	return sm.AddStorage(STORAGE_BACKUP_DB, storage)
}

func (sm *StorageManager) GetStorageBackupDb() Storage {
	return sm.GetStorage(STORAGE_BACKUP_DB)
}

func (sm *StorageManager) AddStorageMapping(storage *ShardelDB) error {
	return sm.AddStorageShardelDB(STORAGE_MAPPING_DB, storage)
}

func (sm *StorageManager) GetStorageMapping() *ShardelDB {
	return sm.GetStorageShardelDB(STORAGE_MAPPING_DB)

}

// CloseAll đóng tất cả các database trong StorageManager
func (sm *StorageManager) CloseAll() error {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	var closeErrs []error

	for dbType, storage := range sm.storages {
		if err := storage.Close(); err != nil {
			closeErrs = append(closeErrs, errors.New(dbType.String()+": "+err.Error()))
		}
		delete(sm.storages, dbType) // Xóa khỏi danh sách sau khi đóng
	}

	for dbType, storage := range sm.shardelDB {
		if err := storage.Close(); err != nil {
			closeErrs = append(closeErrs, errors.New(dbType.String()+": "+err.Error()))
		}
		delete(sm.storages, dbType) // Xóa khỏi danh sách sau khi đóng
	}

	// Nếu có lỗi khi đóng, gộp tất cả lại và trả về
	if len(closeErrs) > 0 {
		return errors.New("failed to close some storages: " + combineErrors(closeErrs))
	}

	return nil
}

// combineErrors nối lỗi thành một chuỗi duy nhất
func combineErrors(errs []error) string {
	var result string
	for _, err := range errs {
		if result != "" {
			result += "; "
		}
		result += err.Error()
	}
	return result
}
