package network

import (
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/meta-node-blockchain/meta-node/types/network"
)

// Handler chịu trách nhiệm xử lý các request đến dựa trên các route đã đăng ký.
// Nó cũng hỗ trợ giới hạn tần suất (rate limiting) cho mỗi route.
type Handler struct {
	routes   map[string]func(network.Request) error // routes ánh xạ tên lệnh tới hàm xử lý tương ứng.
	limits   map[string]int                         // limits ánh xạ tên lệnh tới số lần gọi tối đa mỗi giây.
	requests map[string]int                         // requests theo dõi số lần gọi hiện tại cho mỗi lệnh trong khoảng thời gian 1 giây.
	mutex    sync.Mutex                             // mutex bảo vệ truy cập đồng thời vào map 'requests'.
	// Cân nhắc thêm một context cho Handler để có thể dừng các goroutine của rate limiter khi Handler không còn dùng nữa.
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
	if limits == nil {
		limits = make(map[string]int)
	}
	return &Handler{
		routes:   routes,
		limits:   limits,
		requests: make(map[string]int),
	}
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

	// Kiểm tra giới hạn số lần gọi
	h.mutex.Lock()
	limit, limitExists := h.limits[cmd]
	if limitExists {
		currentRequests := h.requests[cmd]
		if currentRequests >= limit {
			h.mutex.Unlock()
			return fmt.Errorf("vượt quá giới hạn tần suất cho lệnh: %s (giới hạn %d req/s)", cmd, limit)
		}
		h.requests[cmd] = currentRequests + 1
	}
	h.mutex.Unlock()

	if limitExists {
		// TODO: Cân nhắc sử dụng một Ticker toàn cục thay vì một goroutine cho mỗi request để quản lý rate limiting.
		// Điều này sẽ hiệu quả hơn với lượng request lớn.
		go func(commandToDecrement string) {
			time.Sleep(time.Second)
			h.mutex.Lock()
			// Chỉ giảm nếu giá trị vẫn lớn hơn 0
			if val, ok := h.requests[commandToDecrement]; ok && val > 0 {
				h.requests[commandToDecrement]--
			}
			// Nếu giá trị là 0, có thể xóa key khỏi map để tiết kiệm bộ nhớ nếu lệnh không thường xuyên được gọi.
			// else if ok && val == 0 {
			// delete(h.requests, commandToDecrement)
			// }
			h.mutex.Unlock()
		}(cmd)
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
