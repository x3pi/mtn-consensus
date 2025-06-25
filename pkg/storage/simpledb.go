package storage

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
)

const (
	KeySize    = 32          // Kích thước cố định của key
	IndexEntry = KeySize + 8 // Mỗi entry trong file index gồm key (32 bytes) + offset (8 bytes)
)

// SimpleDB đại diện cho một cơ sở dữ liệu đơn giản
type SimpleDB struct {
	file      *os.File         // File lưu trữ dữ liệu
	indexFile *os.File         // File lưu trữ index (hash table)
	indexMap  map[string]int64 // Hash table lưu key và offset
	mu        sync.RWMutex     // Khóa để đảm bảo đồng bộ hóa
	dbPath    string           // Đường dẫn đến file dữ liệu
	indexPath string           // Đường dẫn đến file index
}

// SimpleDBManager quản lý nhiều instances của SimpleDB
type SimpleDBManager struct {
	databases map[string]*SimpleDB // Map lưu trữ các database đang mở
	mu        sync.RWMutex         // Khóa để đảm bảo đồng bộ hóa
}

// NewSimpleDBManager tạo một instance mới của SimpleDBManager
func NewSimpleDBManager() *SimpleDBManager {
	return &SimpleDBManager{
		databases: make(map[string]*SimpleDB),
	}
}

// GetOrCreate mở hoặc tạo mới một database với đường dẫn cơ sở
func (mgr *SimpleDBManager) GetOrCreate(basePath string) (*SimpleDB, error) {
	mgr.mu.Lock()
	defer mgr.mu.Unlock()

	// Kiểm tra xem database đã được mở chưa
	if db, exists := mgr.databases[basePath]; exists {
		return db, nil
	}

	// Tạo đường dẫn cho file index
	indexPath := basePath + "_index"

	// Tạo thư mục nếu chưa tồn tại
	err := os.MkdirAll(filepath.Dir(basePath), os.ModePerm)
	if err != nil {
		return nil, fmt.Errorf("lỗi khi tạo thư mục: %w", err)
	}

	// Tạo thư mục cho file index nếu chưa tồn tại
	err = os.MkdirAll(filepath.Dir(indexPath), os.ModePerm)
	if err != nil {
		return nil, fmt.Errorf("lỗi khi tạo thư mục index: %w", err)
	}

	// Kiểm tra xem file database có tồn tại không
	if _, err := os.Stat(basePath); os.IsNotExist(err) {
		// Nếu file database không tồn tại, tạo mới
		file, err := os.Create(basePath)
		if err != nil {
			return nil, fmt.Errorf("lỗi khi tạo file database: %w", err)
		}
		file.Close()
	}

	// Kiểm tra xem file index có tồn tại không
	if _, err := os.Stat(indexPath); os.IsNotExist(err) {
		// Nếu file index không tồn tại, tạo mới
		indexFile, err := os.Create(indexPath)
		if err != nil {
			return nil, fmt.Errorf("lỗi khi tạo file index: %w", err)
		}
		indexFile.Close()
	}

	// Mở hoặc tạo mới database
	db, err := Open(basePath, indexPath)
	if err != nil {
		return nil, err
	}

	// Lưu database vào manager
	mgr.databases[basePath] = db

	return db, nil
}

// Close đóng một database
func (mgr *SimpleDBManager) Close(basePath string) error {
	mgr.mu.Lock()
	defer mgr.mu.Unlock()

	// Kiểm tra xem database có tồn tại không
	db, exists := mgr.databases[basePath]
	if !exists {
		return fmt.Errorf("database không tồn tại: %s", basePath)
	}

	// Đóng database
	err := db.Close()
	if err != nil {
		return err
	}

	// Xóa database khỏi manager
	delete(mgr.databases, basePath)

	return nil
}

// CloseAll đóng tất cả các database đang mở
func (mgr *SimpleDBManager) CloseAll() error {
	mgr.mu.Lock()
	defer mgr.mu.Unlock()

	// Đóng từng database
	for basePath, db := range mgr.databases {
		err := db.Close()
		if err != nil {
			return fmt.Errorf("lỗi khi đóng database %s: %w", basePath, err)
		}
	}

	// Xóa tất cả database khỏi manager
	mgr.databases = make(map[string]*SimpleDB)

	return nil
}

// Open mở hoặc tạo mới một database
func Open(dbPath, indexPath string) (*SimpleDB, error) {
	// Mở file dữ liệu
	file, err := os.OpenFile(dbPath, os.O_RDWR|os.O_CREATE, 0666)
	if err != nil {
		return nil, fmt.Errorf("lỗi khi mở file DB: %w", err)
	}

	// Mở file index
	indexFile, err := os.OpenFile(indexPath, os.O_RDWR|os.O_CREATE, 0666)
	if err != nil {
		return nil, fmt.Errorf("lỗi khi mở file index: %w", err)
	}

	db := &SimpleDB{
		file:      file,
		indexFile: indexFile,
		indexMap:  make(map[string]int64),
		dbPath:    dbPath,
		indexPath: indexPath,
	}

	// Khôi phục hash table từ file index
	err = db.loadIndex()
	if err != nil {
		return nil, fmt.Errorf("lỗi khi tải index: %w", err)
	}

	return db, nil
}

// Đóng database
func (db *SimpleDB) Close() error {
	// db.Compact()
	// Lưu hash table vào file index trước khi đóng
	err := db.saveIndex()
	if err != nil {
		return fmt.Errorf("lỗi khi lưu index: %w", err)
	}

	// Đóng file dữ liệu
	err = db.file.Close()
	if err != nil {
		return fmt.Errorf("lỗi khi đóng file DB: %w", err)
	}

	// Đóng file index
	err = db.indexFile.Close()
	if err != nil {
		return fmt.Errorf("lỗi khi đóng file index: %w", err)
	}

	return nil
}

// Lưu hash table vào file index
func (db *SimpleDB) saveIndex() error {
	// db.mu.Lock()
	// defer db.mu.Unlock()

	// Xóa nội dung cũ của file index
	err := db.indexFile.Truncate(0)
	if err != nil {
		return fmt.Errorf("lỗi khi xóa nội dung file index: %w", err)
	}

	// Di chuyển con trỏ về đầu file
	_, err = db.indexFile.Seek(0, io.SeekStart)
	if err != nil {
		return fmt.Errorf("lỗi khi di chuyển con trỏ về đầu file index: %w", err)
	}

	// Ghi từng cặp key và offset vào file index
	for key, offset := range db.indexMap {
		// Ghi key (32 bytes)
		_, err := db.indexFile.Write([]byte(key))
		if err != nil {
			return fmt.Errorf("lỗi khi ghi key vào file index: %w", err)
		}

		// Ghi offset (8 bytes)
		err = binary.Write(db.indexFile, binary.LittleEndian, offset)
		if err != nil {
			return fmt.Errorf("lỗi khi ghi offset vào file index: %w", err)
		}
	}

	// Đảm bảo dữ liệu được flush xuống đĩa
	err = db.indexFile.Sync()
	if err != nil {
		return fmt.Errorf("lỗi khi flush dữ liệu xuống đĩa: %w", err)
	}

	return nil
}

// Tải hash table từ file index
func (db *SimpleDB) loadIndex() error {
	// db.mu.Lock()
	// defer db.mu.Unlock()

	// Đọc toàn bộ nội dung file index
	fileInfo, err := db.indexFile.Stat()
	if err != nil {
		return fmt.Errorf("lỗi khi lấy thông tin file index: %w", err)
	}
	fileSize := fileInfo.Size()

	// Di chuyển con trỏ về đầu file
	_, err = db.indexFile.Seek(0, io.SeekStart)
	if err != nil {
		return fmt.Errorf("lỗi khi di chuyển con trỏ về đầu file index: %w", err)
	}

	// Đọc từng entry trong file index
	for offset := int64(0); offset < fileSize; offset += IndexEntry {
		// Đọc key (32 bytes)
		key := make([]byte, KeySize)
		_, err := db.indexFile.Read(key)
		if err != nil {
			if err == io.EOF {
				break
			}
			return fmt.Errorf("lỗi khi đọc key từ file index: %w", err)
		}

		// Đọc offset (8 bytes)
		var valueOffset int64
		err = binary.Read(db.indexFile, binary.LittleEndian, &valueOffset)
		if err != nil {
			return fmt.Errorf("lỗi khi đọc offset từ file index: %w", err)
		}

		// Lưu key và offset vào hash table
		db.indexMap[string(key)] = valueOffset
	}

	return nil
}

// func (db *SimpleDB) Put(key, value []byte) error {
// 	if len(key) != KeySize {
// 		return errors.New("key phải có đúng 32 bytes")
// 	}

// 	db.mu.Lock()
// 	defer db.mu.Unlock()

// 	// Đánh dấu dữ liệu cũ là đã xóa (tombstone)
// 	if offset, exists := db.indexMap[string(key)]; exists {
// 		_, err := db.file.Seek(offset, io.SeekStart)
// 		if err != nil {
// 			return fmt.Errorf("lỗi khi di chuyển đến offset: %w", err)
// 		}

// 		// Ghi tombstone (ví dụ: giá trị độ dài là 0)
// 		err = binary.Write(db.file, binary.LittleEndian, uint32(0))
// 		if err != nil {
// 			return fmt.Errorf("lỗi khi ghi tombstone vào file DB: %w", err)
// 		}
// 	}

// 	// Ghi dữ liệu mới vào cuối file
// 	offset, err := db.file.Seek(0, io.SeekEnd)
// 	if err != nil {
// 		return fmt.Errorf("lỗi khi tìm kiếm cuối file DB: %w", err)
// 	}

// 	// Ghi độ dài value (4 bytes)
// 	valueSize := uint32(len(value))
// 	err = binary.Write(db.file, binary.LittleEndian, valueSize)
// 	if err != nil {
// 		return fmt.Errorf("lỗi khi ghi độ dài value vào file DB: %w", err)
// 	}

// 	// Ghi value
// 	_, err = db.file.Write(value)
// 	if err != nil {
// 		return fmt.Errorf("lỗi khi ghi value vào file DB: %w", err)
// 	}

// 	// Ghi key
// 	_, err = db.file.Write(key)
// 	if err != nil {
// 		return fmt.Errorf("lỗi khi ghi key vào file DB: %w", err)
// 	}

// 	// Cập nhật hash table
// 	db.indexMap[string(key)] = offset

// 	// Lưu index
// 	err = db.saveIndex()
// 	if err != nil {
// 		return fmt.Errorf("lỗi khi lưu index: %w", err)
// 	}

// 	return nil
// }

func (db *SimpleDB) Put(key, value []byte) error {
	if len(key) != KeySize {
		return errors.New("key phải có đúng 32 bytes")
	}

	db.mu.Lock()
	defer db.mu.Unlock()

	// Xóa key cũ nếu tồn tại
	delete(db.indexMap, string(key))

	// Ghi dữ liệu mới vào cuối file
	offset, err := db.file.Seek(0, io.SeekEnd)
	if err != nil {
		return fmt.Errorf("lỗi khi tìm kiếm cuối file DB: %w", err)
	}

	// Ghi độ dài value (4 bytes)
	valueSize := uint32(len(value))
	err = binary.Write(db.file, binary.LittleEndian, valueSize)
	if err != nil {
		return fmt.Errorf("lỗi khi ghi độ dài value vào file DB: %w", err)
	}

	// Ghi value
	_, err = db.file.Write(value)
	if err != nil {
		return fmt.Errorf("lỗi khi ghi value vào file DB: %w", err)
	}

	// Ghi key
	_, err = db.file.Write(key)
	if err != nil {
		return fmt.Errorf("lỗi khi ghi key vào file DB: %w", err)
	}

	// Cập nhật hash table
	db.indexMap[string(key)] = offset

	// Lưu index
	err = db.saveIndex()
	if err != nil {
		return fmt.Errorf("lỗi khi lưu index: %w", err)
	}

	return nil
}

func (db *SimpleDB) Compact() error {
	db.mu.Lock()
	defer db.mu.Unlock()

	// Tạo file tạm thời
	tempFilePath := db.dbPath + ".tmp"
	tempFile, err := os.Create(tempFilePath)
	if err != nil {
		return fmt.Errorf("lỗi khi tạo file tạm: %w", err)
	}
	defer tempFile.Close()

	// Duyệt qua indexMap và ghi lại dữ liệu hợp lệ vào file tạm
	newIndexMap := make(map[string]int64)
	for key, offset := range db.indexMap {
		// Đọc dữ liệu từ file cũ
		_, err := db.file.Seek(offset, io.SeekStart)
		if err != nil {
			return fmt.Errorf("lỗi khi di chuyển đến offset: %w", err)
		}

		// Đọc độ dài value
		var valueSize uint32
		err = binary.Read(db.file, binary.LittleEndian, &valueSize)
		if err != nil {
			return fmt.Errorf("lỗi khi đọc độ dài value: %w", err)
		}

		// Đọc value
		value := make([]byte, valueSize)
		_, err = db.file.Read(value)
		if err != nil {
			return fmt.Errorf("lỗi khi đọc value: %w", err)
		}

		// Ghi dữ liệu vào file tạm
		newOffset, err := tempFile.Seek(0, io.SeekEnd)
		if err != nil {
			return fmt.Errorf("lỗi khi tìm kiếm cuối file tạm: %w", err)
		}

		// Ghi độ dài value
		err = binary.Write(tempFile, binary.LittleEndian, valueSize)
		if err != nil {
			return fmt.Errorf("lỗi khi ghi độ dài value vào file tạm: %w", err)
		}

		// Ghi value
		_, err = tempFile.Write(value)
		if err != nil {
			return fmt.Errorf("lỗi khi ghi value vào file tạm: %w", err)
		}

		// Ghi key
		_, err = tempFile.Write([]byte(key))
		if err != nil {
			return fmt.Errorf("lỗi khi ghi key vào file tạm: %w", err)
		}

		// Cập nhật indexMap mới
		newIndexMap[key] = newOffset
	}

	// Đóng file tạm
	err = tempFile.Close()
	if err != nil {
		return fmt.Errorf("lỗi khi đóng file tạm: %w", err)
	}

	// Đóng file cũ
	err = db.file.Close()
	if err != nil {
		return fmt.Errorf("lỗi khi đóng file DB cũ: %w", err)
	}

	// Xóa file cũ và đổi tên file tạm thành file chính
	err = os.Remove(db.dbPath)
	if err != nil {
		return fmt.Errorf("lỗi khi xóa file DB cũ: %w", err)
	}
	err = os.Rename(tempFilePath, db.dbPath)
	if err != nil {
		return fmt.Errorf("lỗi khi đổi tên file tạm: %w", err)
	}

	// Mở lại file mới
	db.file, err = os.OpenFile(db.dbPath, os.O_RDWR|os.O_CREATE, 0666)
	if err != nil {
		return fmt.Errorf("lỗi khi mở file DB mới: %w", err)
	}

	// Cập nhật indexMap
	db.indexMap = newIndexMap

	// Lưu index
	err = db.saveIndex()
	if err != nil {
		return fmt.Errorf("lỗi khi lưu index: %w", err)
	}

	return nil
}

// BatchPut thêm nhiều cặp key-value vào database cùng một lúc
func (db *SimpleDB) BatchPut(kvs [][2][]byte) error {
	db.mu.Lock()
	defer db.mu.Unlock()

	// Duyệt qua từng cặp key-value
	for _, kv := range kvs {
		key := kv[0]
		value := kv[1]

		// Kiểm tra kích thước key
		if len(key) != KeySize {
			return fmt.Errorf("key phải có đúng %d bytes", KeySize)
		}

		// Ghi vào file dữ liệu
		offset, err := db.file.Seek(0, io.SeekEnd)
		if err != nil {
			return fmt.Errorf("lỗi khi tìm kiếm cuối file DB: %w", err)
		}

		// Ghi độ dài value (4 bytes)
		valueSize := uint32(len(value))
		err = binary.Write(db.file, binary.LittleEndian, valueSize)
		if err != nil {
			return fmt.Errorf("lỗi khi ghi độ dài value vào file DB: %w", err)
		}

		// Ghi value
		_, err = db.file.Write(value)
		if err != nil {
			return fmt.Errorf("lỗi khi ghi value vào file DB: %w", err)
		}

		// Ghi key
		_, err = db.file.Write(key)
		if err != nil {
			return fmt.Errorf("lỗi khi ghi key vào file DB: %w", err)
		}

		// Cập nhật hash table
		db.indexMap[string(key)] = offset
	}
	db.saveIndex()

	return nil
}

func (db *SimpleDB) Get(key []byte) ([]byte, error) {
	if len(key) != KeySize {
		return nil, errors.New("key phải có đúng 32 bytes")
	}

	db.mu.RLock()
	offset, found := db.indexMap[string(key)]
	db.mu.RUnlock()

	if !found {
		return nil, errors.New("không tìm thấy key")
	}

	// Đọc độ dài value (4 bytes)
	var valueSize uint32
	_, err := db.file.Seek(offset, io.SeekStart)
	if err != nil {
		return nil, fmt.Errorf("lỗi khi di chuyển đến offset: %w", err)
	}

	err = binary.Read(db.file, binary.LittleEndian, &valueSize)
	if err != nil {
		if err == io.EOF {
			return nil, fmt.Errorf("lỗi EOF khi đọc độ dài value từ file DB: %w", err)
		}
		return nil, fmt.Errorf("lỗi khi đọc độ dài value từ file DB: %w", err)
	}

	// Đọc value
	value := make([]byte, valueSize)
	_, err = db.file.Read(value)
	if err != nil {
		if err == io.EOF {
			return nil, fmt.Errorf("lỗi EOF khi đọc value từ file DB: %w", err)
		}
		return nil, fmt.Errorf("lỗi khi đọc value từ file DB: %w", err)
	}

	return value, nil
}

// Xóa key
func (db *SimpleDB) Delete(key []byte) error {
	if len(key) != KeySize {
		return errors.New("key phải có đúng 32 bytes")
	}

	db.mu.Lock()
	defer db.mu.Unlock()

	// Xóa key khỏi hash table
	delete(db.indexMap, string(key))

	return nil
}
