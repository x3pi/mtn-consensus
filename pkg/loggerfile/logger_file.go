package loggerfile

import (
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// Global log directory configuration
var globalLogDir = "logs"

// SetGlobalLogDir sets the global log directory
func SetGlobalLogDir(logDir string) {
	globalLogDir = logDir
}

// GetGlobalLogDir returns the current global log directory
func GetGlobalLogDir() string {
	return globalLogDir
}

// FileLogger struct quản lý ghi log vào file
type FileLogger struct {
	file    *os.File
	entries []LogEntry
	mutex   sync.Mutex
}

// NewFileLogger tạo mới một FileLogger
func NewFileLogger(filePath string) (*FileLogger, error) {
	// Tạo thư mục logs nếu chưa tồn tại
	logDir := globalLogDir
	if _, err := os.Stat(logDir); os.IsNotExist(err) {
		err := os.MkdirAll(logDir, os.ModePerm)
		if err != nil {
			return nil, fmt.Errorf("failed to create logs directory: %w", err)
		}
	}

	// Tạo thư mục con nếu chưa tồn tại
	dir := filepath.Dir(fmt.Sprintf("%s/%s", logDir, filePath))
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		err := os.MkdirAll(dir, os.ModePerm)
		if err != nil {
			return nil, fmt.Errorf("failed to create sub logs directory: %w", err)
		}
	}

	file, err := os.OpenFile(fmt.Sprintf("%s/%s", logDir, filePath), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return nil, fmt.Errorf("failed to open log file: %w", err)
	}
	return &FileLogger{
		file:    file,
		entries: make([]LogEntry, 0),
	}, nil
}

// Log ghi một message đơn giản vào file
func (fl *FileLogger) Log(message string) {
	if fl == nil {
		log.Println("FileLogger is nil. Skipping Log.")
		return
	}

	fl.mutex.Lock()
	defer fl.mutex.Unlock()

	timestamp := time.Now().Format(time.RFC3339)
	logMessage := fmt.Sprintf("%s: %s\n", timestamp, message)
	if _, err := fl.file.WriteString(logMessage); err != nil {
		log.Printf("Failed to write log message: %v", err)
	}
}

// Info ghi một message định dạng vào file
func (fl *FileLogger) Info(message interface{}, a ...interface{}) {
	if fl == nil {
		log.Println("FileLogger is nil. Skipping Info log.")
		return
	}

	fl.mutex.Lock()
	defer fl.mutex.Unlock()

	timestamp := time.Now().Format(time.RFC3339)
	logMessage := fmt.Sprintf("%s: %s\n", timestamp, fmt.Sprintf(fmt.Sprint(message), a...))
	if _, err := fl.file.WriteString(logMessage); err != nil {
		log.Printf("Failed to write log message: %v", err)
	}
}

// LogEntry lưu trữ một mục log chi tiết
type LogEntry struct {
	StartTime                     time.Time `json:"timestamp"`
	GenerateBlockTime             float64   `json:"GenerateBlockTime"`
	ProcessTransactionsInPoolTime float64   `json:"ProcessTransactionsInPoolTime"`
	CreateBlockTime               float64   `json:"CreateBlockTime"`
	LenTxs                        int       `json:"LenTxs"`
	Block                         string    `json:"Block"`
	Txs                           string    `json:"Txs"`
}

// Flush ghi các mục log từ bộ nhớ vào file
func (fl *FileLogger) Flush() {
	if fl == nil {
		log.Println("FileLogger is nil. Skipping Flush.")
		return
	}

	fl.mutex.Lock()
	defer fl.mutex.Unlock()

	if len(fl.entries) == 0 {
		return // Không có dữ liệu để ghi
	}

	data, err := json.MarshalIndent(fl.entries, "", "  ")
	if err != nil {
		log.Printf("Error marshaling JSON: %v", err)
		return
	}

	data = append(data, '\n')
	if _, err := fl.file.Write(data); err != nil {
		log.Printf("Error writing to file: %v", err)
		return
	}
	fl.entries = fl.entries[:0] // Xóa dữ liệu sau khi ghi
}

// FlushPeriodically thực hiện Flush theo chu kỳ
func (fl *FileLogger) FlushPeriodically(interval time.Duration) {
	if fl == nil {
		log.Println("FileLogger is nil. Skipping FlushPeriodically.")
		return
	}

	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	for range ticker.C {
		fl.Flush()
	}
}

// Close đóng file log
func (fl *FileLogger) Close() {
	if fl == nil {
		log.Println("FileLogger is nil. Skipping Close.")
		return
	}

	if err := fl.file.Close(); err != nil {
		log.Printf("Error closing file: %v", err)
	}
}
