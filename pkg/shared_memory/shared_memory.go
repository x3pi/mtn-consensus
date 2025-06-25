package sharedmemory

import (
	// "fmt"
	"sync"
)

// SharedMemory sử dụng sync.Map để quản lý dữ liệu an toàn trong môi trường đa luồng
type SharedMemory struct {
	data sync.Map
}

// Biến toàn cục để truy cập SharedMemory
var GlobalSharedMemory = NewSharedMemory()

// NewSharedMemory khởi tạo một đối tượng SharedMemory
func NewSharedMemory() *SharedMemory {
	return &SharedMemory{}
}

// Write lưu trữ một giá trị vào bộ nhớ chia sẻ với khóa cụ thể
func (sm *SharedMemory) Write(key string, value interface{}) {
	sm.data.Store(key, value)
}

// Read đọc giá trị từ bộ nhớ chia sẻ với khóa cụ thể
func (sm *SharedMemory) Read(key string) (interface{}, bool) {
	return sm.data.Load(key)
}

// Delete xóa một mục khỏi bộ nhớ chia sẻ
func (sm *SharedMemory) Delete(key string) {
	sm.data.Delete(key)
}

// List liệt kê tất cả các mục hiện có trong bộ nhớ chia sẻ
func (sm *SharedMemory) List() map[string]interface{} {
	items := make(map[string]interface{})
	sm.data.Range(func(key, value interface{}) bool {
		items[key.(string)] = value
		return true
	})
	return items
}

// Clear xóa tất cả các mục trong bộ nhớ chia sẻ
func (sm *SharedMemory) Clear() {
	sm.data.Range(func(key, _ interface{}) bool {
		sm.data.Delete(key)
		return true
	})
}
