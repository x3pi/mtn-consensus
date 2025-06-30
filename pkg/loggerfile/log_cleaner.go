package loggerfile

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
)

// LogCleaner quản lý việc xóa logs tự động
type LogCleaner struct {
	logDir string
	stop   chan struct{}
}

// NewLogCleaner tạo mới một LogCleaner
func NewLogCleaner(logDir string) *LogCleaner {
	return &LogCleaner{
		logDir: logDir,
		stop:   make(chan struct{}),
	}
}

// CleanLogs xóa tất cả file logs trong thư mục
func (lc *LogCleaner) CleanLogs() error {
	logger.Info("Bắt đầu xóa logs...")

	// Kiểm tra thư mục logs có tồn tại không
	if _, err := os.Stat(lc.logDir); os.IsNotExist(err) {
		logger.Info("Thư mục logs không tồn tại, bỏ qua việc xóa")
		return nil
	}

	// Xóa tất cả file trong thư mục logs
	err := filepath.Walk(lc.logDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}

		// Bỏ qua thư mục gốc
		if path == lc.logDir {
			return nil
		}

		// Nếu là file, xóa file
		if !info.IsDir() {
			if err := os.Remove(path); err != nil {
				logger.Error(fmt.Sprintf("Không thể xóa file %s: %v", path, err))
				return err
			}
			logger.Info(fmt.Sprintf("Đã xóa file: %s", path))
		} else {
			// Nếu là thư mục, xóa thư mục và tất cả nội dung bên trong
			if err := os.RemoveAll(path); err != nil {
				logger.Error(fmt.Sprintf("Không thể xóa thư mục %s: %v", path, err))
				return err
			}
			logger.Info(fmt.Sprintf("Đã xóa thư mục: %s", path))
		}

		return nil
	})

	if err != nil {
		logger.Error(fmt.Sprintf("Lỗi khi xóa logs: %v", err))
		return err
	}

	logger.Info("Hoàn thành xóa logs")
	return nil
}

// StartDailyCleanup bắt đầu lịch trình xóa logs hàng ngày lúc 12h múi giờ 7
func (lc *LogCleaner) StartDailyCleanup() {
	go func() {
		for {
			// Tính thời gian đến 12h ngày hôm sau múi giờ 7 (UTC+7)
			now := time.Now()
			location, err := time.LoadLocation("Asia/Bangkok")
			if err != nil {
				location = time.FixedZone("UTC+7", 7*60*60)
			}

			nowInLocation := now.In(location)
			nextCleanup := time.Date(nowInLocation.Year(), nowInLocation.Month(), nowInLocation.Day(), 0, 0, 0, 0, location)

			// Nếu đã qua 00h hôm nay, lên lịch cho ngày mai
			if nowInLocation.Hour() >= 0 {
				nextCleanup = nextCleanup.Add(24 * time.Hour)
			}

			// Tính thời gian chờ
			waitDuration := nextCleanup.Sub(nowInLocation)
			logger.Info(fmt.Sprintf("Lịch trình xóa logs tiếp theo: %s (sau %v)", nextCleanup.Format("2006-01-02 15:04:05"), waitDuration))

			// Chờ đến thời gian xóa
			select {
			case <-time.After(waitDuration):
				if err := lc.CleanLogs(); err != nil {
					logger.Error(fmt.Sprintf("Lỗi khi xóa logs tự động: %v", err))
				}
			case <-lc.stop:
				logger.Info("Dừng lịch trình xóa logs hàng ngày")
				return
			}
		}
	}()
}

// Stop dừng lịch trình xóa logs
func (lc *LogCleaner) Stop() {
	close(lc.stop)
}
