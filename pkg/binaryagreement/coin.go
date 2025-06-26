package binaryagreement

import (
	"crypto/sha256"
	"fmt"
	"sync"
)

// ThreshSigLib là một interface giả định cho thư viện chữ ký ngưỡng.
type ThreshSigLib interface {
	Sign(data []byte) []byte                                    // Tạo share
	Combine(data []byte, shares map[int][]byte) ([]byte, error) // Kết hợp các share
}

// Coin triển khai cơ chế đồng xu chung.
type Coin struct {
	mu         sync.Mutex
	pid        string
	round      int64
	sigLib     ThreshSigLib
	shares     map[int][]byte
	decision   *bool
	quorumSize int
	started    bool // <-- Thêm trường này để theo dõi trạng thái kích hoạt
}

// NewCoin tạo một thể hiện Coin mới.
func NewCoin(pid string, round int64, sigLib ThreshSigLib, quorumSize int) *Coin {
	return &Coin{
		pid:        pid,
		round:      round,
		sigLib:     sigLib,
		shares:     make(map[int][]byte),
		quorumSize: quorumSize,
		started:    false, // Giá trị khởi tạo là false
	}
}

// getCoinName tạo ra một định danh duy nhất cho đồng xu của vòng hiện tại.
func (c *Coin) getCoinName() []byte {
	return []byte(fmt.Sprintf("%s-%d", c.pid, c.round))
}

// GetMyShare tạo ra phần đóng góp (share) của nút hiện tại.
func (c *Coin) GetMyShare() []byte {
	return c.sigLib.Sign(c.getCoinName())
}

// AddShare thêm một share từ nút khác và thử quyết định giá trị của đồng xu.
func (c *Coin) AddShare(senderID int, share []byte) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.decision != nil {
		return // Đã quyết định
	}

	if _, exists := c.shares[senderID]; !exists {
		c.shares[senderID] = share
	}

	if len(c.shares) >= c.quorumSize {
		fullSig, err := c.sigLib.Combine(c.getCoinName(), c.shares)
		if err == nil {
			hash := sha256.Sum256(fullSig)
			decision := (hash[0] & 1) == 1
			c.decision = &decision
		}
	}
}

// HasDecided trả về true nếu đồng xu đã có kết quả.
func (c *Coin) HasDecided() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.decision != nil
}

// Value trả về kết quả của đồng xu.
func (c *Coin) Value() (bool, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.decision != nil {
		return *c.decision, true
	}
	return false, false
}

// StartOnce kiểm tra nếu chưa bắt đầu thì đánh dấu là đã bắt đầu và trả về true.
// Nếu đã bắt đầu rồi thì trả về false. Điều này đảm bảo logic kích hoạt chỉ chạy một lần.
// Đây là phương thức đã bị thiếu.
func (c *Coin) StartOnce() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.started {
		return false // Đã bắt đầu rồi, không làm gì cả
	}
	c.started = true
	return true // Đánh dấu đã bắt đầu thành công
}
