package storage

import (
	fmt "fmt"
	"sync/atomic"
)

var (
	lastBlockNumber           uint64
	updateState               uint32
	firstUpdateInRam          uint64
	firstUpdateInDb           uint64
	connectState              uint32
	lastBlockNumberFromMaster uint64
)

// Định nghĩa các trạng thái cập nhật
const (
	DoneSubscribe         uint32 = 1 // Đăng ký hoàn thành
	StateLoadingSnapshot  uint32 = 2 // Đang tải snapshot
	StateSnapshotLoaded   uint32 = 3 // Đã tải xong snapshot
	StateDBReadCompleted  uint32 = 4 // Đọc xong tất cả dữ liệu trong DB
	StateRAMReadCompleted uint32 = 5 // Đọc xong dữ liệu trong RAM

)

var StateChangeChan = make(chan uint32)
var ConnectChangeChan = make(chan uint32)

var commitLock uint32 // 0 = false, 1 = true

// Đặt trạng thái CommitLock và in debug log
func SetCommitLock(lock bool) {
	if lock {
		atomic.StoreUint32(&commitLock, 1)
		fmt.Println("CommitLock set to: true")
	} else {
		atomic.StoreUint32(&commitLock, 0)
		fmt.Println("CommitLock set to: false")
	}
}

// Lấy trạng thái CommitLock và in debug log
func GetCommitLock() bool {
	val := atomic.LoadUint32(&commitLock) == 1
	// logger.Info("CommitLock get: %v\n", val)
	return val
}

func UpdateLastBlockNumber(blockNumber uint64) {
	atomic.StoreUint64(&lastBlockNumber, blockNumber)
}

func GetLastBlockNumber() uint64 {
	return atomic.LoadUint64(&lastBlockNumber)
}

func UpdateLastBlockNumberFromMaster(blockNumber uint64) {
	atomic.StoreUint64(&lastBlockNumberFromMaster, blockNumber)
}

func GetLastBlockNumberFromMaster() uint64 {
	return atomic.LoadUint64(&lastBlockNumberFromMaster)
}

// Cập nhật trạng thái và gửi thông báo qua channel nếu thay đổi
func UpdateState(state uint32) {
	atomic.StoreUint32(&updateState, state)
	StateChangeChan <- state
}

// Lấy trạng thái cập nhật
func GetUpdateState() uint32 {
	return atomic.LoadUint32(&updateState)
}

// Cập nhật trạng thái kết nối và gửi thông báo qua channel nếu thay đổi
func UpdateConnectState(state uint32) {
	atomic.StoreUint32(&connectState, state)
	ConnectChangeChan <- state
}

// Lấy trạng thái kết nối
func GetConnectState() uint32 {
	return atomic.LoadUint32(&connectState)
}

// Hàm để lấy giá trị firstUpdateInRam
func GetFirstUpdateInRam() uint64 {
	return atomic.LoadUint64(&firstUpdateInRam)
}

// Hàm để cập nhật giá trị firstUpdateInRam
func UpdateFirstUpdateInRam(firstUpdate uint64) {
	atomic.StoreUint64(&firstUpdateInRam, firstUpdate)
}

// Hàm để lấy giá trị firstUpdateInRam
func GetFirstUpdateInDb() uint64 {
	return atomic.LoadUint64(&firstUpdateInDb)
}

// Hàm để cập nhật giá trị firstUpdateInRam
func UpdateFirstUpdateInDb(firstUpdate uint64) {
	atomic.StoreUint64(&firstUpdateInDb, firstUpdate)
}
