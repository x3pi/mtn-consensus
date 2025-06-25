package network

import (
	"errors"
	"fmt"
	"sync"

	"golang.org/x/time/rate" // Thêm import cho gói rate

	"github.com/meta-node-blockchain/meta-node/types/network"
)

// Handler chịu trách nhiệm xử lý các request đến dựa trên các route đã đăng ký.
// Nó cũng hỗ trợ giới hạn tần suất (rate limiting) hiệu quả bằng thuật toán token bucket.
type Handler struct {
	routes   map[string]func(network.Request) error // routes ánh xạ tên lệnh tới hàm xử lý tương ứng.
	limiters map[string]*rate.Limiter               // limiters ánh xạ tên lệnh tới một đối tượng rate limiter.
	mutex    sync.Mutex                             // mutex bảo vệ truy cập đồng thời vào map 'limiters'.
}

// NewHandler tạo một instance mới của Handler.
// routes: Một map chứa các route xử lý request.
// limits: Một map chứa giới hạn tần suất cho mỗi route (số request mỗi giây). Nếu nil, không có giới hạn nào được áp dụng ban đầu.
func NewHandler(
	routes map[string]func(network.Request) error,
	limits map[string]int,
) *Handler {
	if routes == nil {
		routes = make(map[string]func(network.Request) error)
	}

	h := &Handler{
		routes:   routes,
		limiters: make(map[string]*rate.Limiter),
	}

	// Khởi tạo rate limiter cho mỗi lệnh có trong map limits.
	// Mỗi limiter sẽ cho phép 'limit' sự kiện mỗi giây, với dung lượng bùng nổ (burst) cũng là 'limit'.
	if limits != nil {
		for command, limitPerSecond := range limits {
			if limitPerSecond > 0 {
				h.limiters[command] = rate.NewLimiter(rate.Limit(limitPerSecond), limitPerSecond)
			}
		}
	}

	return h
}

// HandleRequest xử lý một request đến.
// Nó kiểm tra giới hạn tần suất (nếu có) trước khi gọi hàm xử lý tương ứng.
func (h *Handler) HandleRequest(r network.Request) error {
	if r == nil || r.Message() == nil {
		return errors.New("request hoặc message không hợp lệ")
	}
	conn := r.Connection()
	if conn == nil {
		return errors.New("connection của request là nil")
	}

	cmd := r.Message().Command()

	if cmd == "" {
		return errors.New("lệnh không được để trống")
	}

	// Kiểm tra giới hạn số lần gọi một cách hiệu quả
	h.mutex.Lock()
	limiter, exists := h.limiters[cmd]
	h.mutex.Unlock()

	if exists {
		// Phương thức Allow() của limiter sẽ kiểm tra và tiêu thụ một token
		// mà không cần bất kỳ goroutine hay cơ chế sleep nào.
		// Nó an toàn cho việc gọi đồng thời.
		if !limiter.Allow() {
			return fmt.Errorf("vượt quá giới hạn tần suất cho lệnh: %s", cmd)
		}
	}

	// Xử lý request
	route, routeExists := h.routes[cmd]
	if !routeExists {
		return fmt.Errorf("không tìm thấy lệnh: %s", cmd)
	}

	if route == nil {
		// Đây là một trạng thái không hợp lệ, route đã đăng ký nhưng lại là nil.
		return fmt.Errorf("lỗi nội bộ: route cho lệnh '%s' không được cấu hình đúng cách", cmd)
	}
	return route(r)
}
