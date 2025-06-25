package storage

import (
	"bufio" // Thêm thư viện bufio để đọc/ghi có đệm
	"bytes"
	"encoding/binary"
	"encoding/gob"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync" // Thêm thư viện sync để sử dụng sync.Pool
)

// bufferPool dùng để tái sử dụng các đối tượng bytes.Buffer
var bufferPool = sync.Pool{
	New: func() interface{} {
		return new(bytes.Buffer)
	},
}

// getBuffer lấy một buffer từ pool.
func getBuffer() *bytes.Buffer {
	return bufferPool.Get().(*bytes.Buffer)
}

// putBuffer trả một buffer về pool.
func putBuffer(buf *bytes.Buffer) {
	buf.Reset() // Reset buffer trước khi trả về pool
	bufferPool.Put(buf)
}

type BackUpDb struct {
	BockNumber                uint64
	NodeId                    string
	TxBatchPut                []byte
	AccountBatch              []byte
	BockBatch                 []byte
	BockPut                   []byte
	ReceiptBatchPut           []byte
	SmartContractBatch        []byte
	SmartContractStorageBatch []byte
	TrieDatabaseBatchPut      map[string][]byte

	CodeBatchPut  []byte
	MapppingBatch []byte

	DevicePut    []byte
	DeviceDelete []byte
	FullDbLogs   []map[string][]byte
}

// AddToFullDbLogs thêm một map[string][]byte vào FullDbLogs
func (b *BackUpDb) AddToFullDbLogs(data map[string][]byte) {
	b.FullDbLogs = append(b.FullDbLogs, data)
}

// SerializeByteArrays chuyển đổi [][]byte thành []byte, sử dụng buffer từ pool.
func SerializeByteArrays(data [][]byte) ([]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf) // Đảm bảo buffer được trả về pool

	encoder := gob.NewEncoder(buf)
	err := encoder.Encode(data)
	if err != nil {
		return nil, err
	}
	// Phải sao chép dữ liệu từ buffer trước khi nó được reset và tái sử dụng.
	// buf.Bytes() trả về một slice chia sẻ mảng byte nền của buffer.
	// Tạo một slice mới và sao chép đảm bảo tính toàn vẹn dữ liệu sau khi buffer được đưa vào pool.
	serializedData := make([]byte, buf.Len())
	copy(serializedData, buf.Bytes())
	return serializedData, nil
}

// DeserializeByteArrays chuyển đổi []byte trở lại [][]byte, sử dụng buffer từ pool cho đầu vào.
func DeserializeByteArrays(encodedData []byte) ([][]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	// Ghi encodedData vào buffer để gob.Decoder có thể đọc từ đó
	if _, err := buf.Write(encodedData); err != nil {
		return nil, fmt.Errorf("failed to write encodedData to buffer for deserialization: %w", err)
	}

	var data [][]byte
	decoder := gob.NewDecoder(buf) // buf bây giờ hoạt động như một io.Reader
	err := decoder.Decode(&data)
	if err != nil {
		return nil, err
	}
	return data, nil
}

// SerializeBackupDb chuyển đổi BackUpDb thành []byte, sử dụng buffer từ pool.
func SerializeBackupDb(backup BackUpDb) ([]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	encoder := gob.NewEncoder(buf)
	err := encoder.Encode(backup)
	if err != nil {
		return nil, fmt.Errorf("failed to serialize BackUpDb: %w", err)
	}
	serializedData := make([]byte, buf.Len())
	copy(serializedData, buf.Bytes())
	return serializedData, nil
}

// DeserializeBackupDb chuyển đổi []byte thành BackUpDb, sử dụng buffer từ pool cho đầu vào.
func DeserializeBackupDb(encodedData []byte) (BackUpDb, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	if _, err := buf.Write(encodedData); err != nil {
		return BackUpDb{}, fmt.Errorf("failed to write encodedData to buffer for deserialization: %w", err)
	}

	var backup BackUpDb
	decoder := gob.NewDecoder(buf)
	err := decoder.Decode(&backup)
	if err != nil {
		return BackUpDb{}, fmt.Errorf("failed to deserialize BackUpDb: %w", err)
	}
	return backup, nil
}

// SerializeBatch chuyển đổi [][2][]byte thành []byte, sử dụng buffer từ pool.
func SerializeBatch(data [][2][]byte) ([]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	encoder := gob.NewEncoder(buf)
	err := encoder.Encode(data)
	if err != nil {
		return nil, err
	}
	serializedData := make([]byte, buf.Len())
	copy(serializedData, buf.Bytes())
	return serializedData, nil
}

// DeserializeBatch chuyển đổi []byte trở lại [][2][]byte, sử dụng buffer từ pool cho đầu vào.
func DeserializeBatch(encodedData []byte) ([][2][]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	if _, err := buf.Write(encodedData); err != nil {
		return nil, fmt.Errorf("failed to write encodedData to buffer for deserialization: %w", err)
	}

	var data [][2][]byte
	decoder := gob.NewDecoder(buf)
	err := decoder.Decode(&data)
	if err != nil {
		return nil, err
	}
	return data, nil
}

// AppendBatch thêm dữ liệu batch vào cuối file, với tiền tố kích thước, sử dụng I/O có đệm.
func AppendBatch(filename string, data [][2][]byte) error {
	dir := filepath.Dir(filename)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		if err := os.MkdirAll(dir, os.ModePerm); err != nil {
			return fmt.Errorf("failed to create directory for batch file: %w", err)
		}
	}

	encodedBatchData, err := SerializeBatch(data) // Sử dụng buffer từ pool bên trong
	if err != nil {
		return fmt.Errorf("failed to serialize batch for append: %w", err)
	}

	file, err := os.OpenFile(filename, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0666)
	if err != nil {
		return fmt.Errorf("failed to open batch file for append: %w", err)
	}
	defer file.Close()

	writer := bufio.NewWriter(file) // Sử dụng writer có đệm
	defer writer.Flush()            // Đảm bảo dữ liệu được ghi trước khi đóng

	batchDataSize := int64(len(encodedBatchData))
	if err := binary.Write(writer, binary.LittleEndian, batchDataSize); err != nil {
		return fmt.Errorf("failed to write batch data size: %w", err)
	}

	if _, err := writer.Write(encodedBatchData); err != nil {
		return fmt.Errorf("failed to write batch data: %w", err)
	}

	return nil
}

// LoadBatch khôi phục tất cả các batch đã được append từ file,
// gộp tất cả các cặp key-value thành một slice [][2][]byte duy nhất, sử dụng I/O có đệm.
func LoadBatch(filename string) ([][2][]byte, error) {
	file, err := os.Open(filename)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, os.ErrNotExist
		}
		return nil, fmt.Errorf("failed to open batch file for loading: %w", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file) // Sử dụng reader có đệm
	var finalCombinedBatch [][2][]byte

	for {
		var batchDataSize int64
		if err := binary.Read(reader, binary.LittleEndian, &batchDataSize); err != nil {
			if err == io.EOF {
				break
			}
			return nil, fmt.Errorf("failed to read batch data size: %w", err)
		}

		if batchDataSize <= 0 || batchDataSize > 50*1024*1024 { // Giới hạn 50MB
			return nil, fmt.Errorf("invalid batch data size: %d (must be > 0 and <= 50MB)", batchDataSize)
		}

		encodedBatchData := make([]byte, batchDataSize)
		if _, err := io.ReadFull(reader, encodedBatchData); err != nil {
			return nil, fmt.Errorf("failed to read batch data (expected %d bytes): %w", batchDataSize, err)
		}

		currentBatch, err := DeserializeBatch(encodedBatchData) // Sử dụng buffer từ pool bên trong
		if err != nil {
			return nil, fmt.Errorf("failed to deserialize batch data: %w", err)
		}
		finalCombinedBatch = append(finalCombinedBatch, currentBatch...)
	}

	return finalCombinedBatch, nil
}

// AppendPut ghi một cặp key-value vào file, sử dụng I/O có đệm.
func AppendPut(filename string, key, value []byte) error {
	dir := filepath.Dir(filename)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		if err := os.MkdirAll(dir, os.ModePerm); err != nil {
			return fmt.Errorf("failed to create directory: %w", err)
		}
	}

	file, err := os.OpenFile(filename, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0666)
	if err != nil {
		return fmt.Errorf("failed to open file: %w", err)
	}
	defer file.Close()

	writer := bufio.NewWriter(file) // Sử dụng writer có đệm
	defer writer.Flush()            // Đảm bảo dữ liệu được ghi

	// Mã hóa sử dụng buffer từ pool thông qua PutToBytes
	entryData, err := PutToBytes(key, value)
	if err != nil {
		return fmt.Errorf("failed to encode data: %w", err)
	}

	size := int64(len(entryData))
	if err := binary.Write(writer, binary.LittleEndian, size); err != nil {
		return fmt.Errorf("failed to write entry size: %w", err)
	}

	if _, err := writer.Write(entryData); err != nil {
		return fmt.Errorf("failed to write entry data: %w", err)
	}

	return nil
}

// PutToBytes chuyển đổi một lệnh put thành []byte, sử dụng buffer từ pool.
func PutToBytes(key, value []byte) ([]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	encoder := gob.NewEncoder(buf)
	if err := encoder.Encode([2][]byte{key, value}); err != nil {
		return nil, fmt.Errorf("failed to encode data: %w", err)
	}
	serializedData := make([]byte, buf.Len())
	copy(serializedData, buf.Bytes())
	return serializedData, nil
}

// BytesToPut chuyển đổi []byte trở lại thành một cặp key-value, sử dụng buffer từ pool cho đầu vào.
func BytesToPut(encodedData []byte) ([2][]byte, error) {
	buf := getBuffer()
	defer putBuffer(buf)

	if _, err := buf.Write(encodedData); err != nil {
		return [2][]byte{}, fmt.Errorf("failed to write encodedData to buffer for deserialization: %w", err)
	}

	var data [2][]byte
	decoder := gob.NewDecoder(buf)
	err := decoder.Decode(&data)
	if err != nil {
		return [2][]byte{}, fmt.Errorf("failed to decode data: %w", err)
	}
	return data, nil
}

// LoadAllPut đọc toàn bộ các lệnh put từ file, sử dụng I/O có đệm.
func LoadAllPut(filename string) ([][2][]byte, error) {
	file, err := os.Open(filename)
	if err != nil {
		return nil, fmt.Errorf("failed to open file: %w", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file) // Sử dụng reader có đệm
	var allData [][2][]byte

	for {
		var entrySize int64
		if err := binary.Read(reader, binary.LittleEndian, &entrySize); err != nil {
			if err == io.EOF {
				break
			}
			return nil, fmt.Errorf("failed to read entry size: %w", err)
		}

		if entrySize <= 0 || entrySize > 10*1024*1024 { // Giới hạn 10MB
			return nil, fmt.Errorf("invalid entry size: %d", entrySize)
		}

		entryData := make([]byte, entrySize)
		if _, err := io.ReadFull(reader, entryData); err != nil {
			return nil, fmt.Errorf("failed to read entry data: %w", err)
		}

		// Giải mã sử dụng buffer từ pool thông qua BytesToPut
		entry, err := BytesToPut(entryData)
		if err != nil {
			return nil, fmt.Errorf("failed to decode entry data: %w", err)
		}
		allData = append(allData, entry)
	}

	return allData, nil
}

// AppendDelete ghi một key vào file delete, sử dụng I/O có đệm.
func AppendDelete(filename string, key []byte) error {
	dir := filepath.Dir(filename)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		if err := os.MkdirAll(dir, os.ModePerm); err != nil {
			return fmt.Errorf("failed to create directory: %w", err)
		}
	}

	file, err := os.OpenFile(filename, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0666)
	if err != nil {
		return fmt.Errorf("failed to open file: %w", err)
	}
	defer file.Close()

	writer := bufio.NewWriter(file) // Sử dụng writer có đệm
	defer writer.Flush()            // Đảm bảo dữ liệu được ghi

	// Mã hóa key sử dụng buffer từ pool
	buf := getBuffer()
	encoder := gob.NewEncoder(buf)
	if err := encoder.Encode(key); err != nil {
		putBuffer(buf) // Đảm bảo buffer được trả về nếu có lỗi
		return fmt.Errorf("failed to encode key: %w", err)
	}
	entryData := make([]byte, buf.Len())
	copy(entryData, buf.Bytes())
	putBuffer(buf) // Trả buffer về sau khi dữ liệu đã được sao chép

	size := int64(len(entryData))
	if err := binary.Write(writer, binary.LittleEndian, size); err != nil {
		return fmt.Errorf("failed to write entry size: %w", err)
	}

	if _, err := writer.Write(entryData); err != nil {
		return fmt.Errorf("failed to write entry data: %w", err)
	}

	return nil
}

// LoadAllDelete đọc toàn bộ các key delete từ file, sử dụng I/O có đệm.
func LoadAllDelete(filename string) ([][]byte, error) {
	file, err := os.Open(filename)
	if err != nil {
		return nil, fmt.Errorf("failed to open file: %w", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file) // Sử dụng reader có đệm
	var allKeys [][]byte

	for {
		var entrySize int64
		if err := binary.Read(reader, binary.LittleEndian, &entrySize); err != nil {
			if err == io.EOF {
				break
			}
			return nil, fmt.Errorf("failed to read entry size: %w", err)
		}

		if entrySize <= 0 || entrySize > 10*1024*1024 { // Giới hạn 10MB
			return nil, fmt.Errorf("invalid entry size: %d", entrySize)
		}

		entryData := make([]byte, entrySize)
		if _, err := io.ReadFull(reader, entryData); err != nil {
			return nil, fmt.Errorf("failed to read entry data: %w", err)
		}

		// Giải mã key sử dụng buffer từ pool
		inputBuf := getBuffer()
		if _, err := inputBuf.Write(entryData); err != nil {
			putBuffer(inputBuf)
			return nil, fmt.Errorf("failed to write entryData to buffer for deserialization: %w", err)
		}

		decoder := gob.NewDecoder(inputBuf)
		var currentKey []byte
		if err := decoder.Decode(&currentKey); err != nil {
			putBuffer(inputBuf)
			return nil, fmt.Errorf("failed to decode entry data: %w", err)
		}
		putBuffer(inputBuf) // Trả buffer về sau khi giải mã

		allKeys = append(allKeys, currentKey)
	}

	return allKeys, nil
}
