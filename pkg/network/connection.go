package network

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"google.golang.org/protobuf/proto"

	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/types/network"
)

var (
	// ErrDisconnected cho biết kết nối đã bị ngắt.
	ErrDisconnected = errors.New("lỗi: kết nối đã bị ngắt")
	// ErrInvalidMessageLength cho biết độ dài tin nhắn không hợp lệ.
	ErrInvalidMessageLength = errors.New("lỗi: độ dài tin nhắn không hợp lệ")
	// ErrExceedMessageLength cho biết tin nhắn vượt quá giới hạn độ dài cho phép.
	ErrExceedMessageLength = errors.New("lỗi: tin nhắn vượt quá giới hạn độ dài cho phép")
	// ErrNilConnection cho biết đối tượng kết nối là nil.
	ErrNilConnection = errors.New("lỗi: kết nối là nil")
	// ErrRequestChanFull cho biết kênh request đầy và không thể gửi tin nhắn sau khi chờ.
	ErrRequestChanFull = errors.New("lỗi: kênh request đầy sau thời gian chờ")
)

const (
	// MaxMessageLength defines the maximum allowed size for a message (e.g., 1GB).
	MaxMessageLength = 1 << 30 // 1GB
	// DefaultRequestChanSize là kích thước buffer mặc định cho kênh request.
	DefaultRequestChanSize = 100000
	// DefaultErrorChanSize là kích thước buffer mặc định cho kênh error.
	DefaultErrorChanSize = 1
	// defaultWriteTimeout là thời gian chờ mặc định cho các thao tác ghi mạng.
	defaultWriteTimeout = 60 * time.Second
	// defaultRequestChanWaitTimeout là thời gian chờ tối đa khi kênh request đầy.
	defaultRequestChanWaitTimeout = 5 * time.Second
)

// ConnectionFromTcpConnection tạo một đối tượng network.Connection từ một net.Conn (kết nối TCP).
func ConnectionFromTcpConnection(tcpConn net.Conn) (network.Connection, error) {
	if tcpConn == nil {
		return nil, errors.New("ConnectionFromTcpConnection: tcpConn không được là nil")
	}
	return &Connection{
		address:      common.Address{},
		cType:        "",
		tcpConn:      tcpConn,
		requestChan:  make(chan network.Request, DefaultRequestChanSize),
		errorChan:    make(chan error, DefaultErrorChanSize),
		realConnAddr: tcpConn.RemoteAddr().String(),
		connect:      true,
	}, nil
}

// NewConnection tạo một đối tượng network.Connection mới.
func NewConnection(
	address common.Address,
	cType string,
) network.Connection {
	return &Connection{
		address:     address,
		cType:       cType,
		requestChan: make(chan network.Request, DefaultRequestChanSize),
		errorChan:   make(chan error, DefaultErrorChanSize),
		connect:     false,
	}
}

// Connection đại diện cho một kết nối mạng với một node khác.
type Connection struct {
	mu           sync.Mutex
	address      common.Address
	cType        string
	requestChan  chan network.Request
	errorChan    chan error
	tcpConn      net.Conn
	connect      bool
	realConnAddr string
}

// Address trả về địa chỉ Ethereum của node từ xa.
func (c *Connection) Address() common.Address {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.address
}

// TcpLocalAddr trả về địa chỉ cục bộ của kết nối TCP.
func (c *Connection) TcpLocalAddr() net.Addr {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.tcpConn == nil {
		return nil
	}
	return c.tcpConn.LocalAddr()
}

// TcpRemoteAddr trả về địa chỉ từ xa của kết nối TCP.
func (c *Connection) TcpRemoteAddr() net.Addr {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.tcpConn == nil {
		return nil
	}
	return c.tcpConn.RemoteAddr()
}

// ConnectionAddress trả về địa chỉ IP:PORT thực tế của kết nối.
func (c *Connection) ConnectionAddress() (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.connectionAddressInternal()
}

// RequestChan trả về kênh request và kênh error của kết nối.
func (c *Connection) RequestChan() (chan network.Request, chan error) {
	return c.requestChan, c.errorChan
}

// Type trả về loại của kết nối.
func (c *Connection) Type() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.cType
}

// String trả về một chuỗi mô tả đối tượng Connection.
func (c *Connection) String() string {
	connectionAddress, err := c.ConnectionAddress()
	if err != nil {
		connectionAddress = fmt.Sprintf("Lỗi khi lấy địa chỉ: %v", err)
	}
	return fmt.Sprintf(
		"Connection[Địa chỉ Node: %v, Loại: %v, Địa chỉ TCP: %v, Kết nối: %t]",
		c.address.Hex(),
		c.cType,
		connectionAddress,
		c.IsConnect(),
	)
}

// Init khởi tạo hoặc cập nhật địa chỉ và loại của kết nối.
func (c *Connection) Init(
	address common.Address,
	cType string,
) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.address = address
	c.cType = cType
}

// SetRealConnAddr đặt địa chỉ IP:PORT thực tế của kết nối.
func (c *Connection) SetRealConnAddr(realConnAddr string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.realConnAddr = realConnAddr
}

// SendMessage gửi một đối tượng network.Message qua kết nối TCP.
func (c *Connection) SendMessage(message network.Message) error {
	if c == nil {
		return ErrNilConnection
	}

	b, err := message.Marshal()
	if err != nil {
		return fmt.Errorf("SendMessage: lỗi khi marshal tin nhắn '%s': %w", message.Command(), err)
	}

	c.mu.Lock()
	defer c.mu.Unlock()

	if !c.connect || c.tcpConn == nil {
		return fmt.Errorf("SendMessage: kết nối TCP đến %s không hoạt động hoặc là nil (lệnh: '%s')", c.address.Hex(), message.Command())
	}

	deadline := time.Now().Add(defaultWriteTimeout)
	if err := c.tcpConn.SetWriteDeadline(deadline); err != nil {
	}
	defer func() {
		_ = c.tcpConn.SetWriteDeadline(time.Time{})
	}()

	length := make([]byte, 8)
	binary.LittleEndian.PutUint64(length, uint64(len(b)))

	_, err = c.tcpConn.Write(length)
	if err != nil {
		if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
		}
		return fmt.Errorf("SendMessage: lỗi khi gửi độ dài tin nhắn đến %s (lệnh: '%s'): %w", c.address.Hex(), message.Command(), err)
	}

	totalSent := 0
	for totalSent < len(b) {
		n, errWrite := c.tcpConn.Write(b[totalSent:])
		if errWrite != nil {
			if netErr, ok := errWrite.(net.Error); ok && netErr.Timeout() {
			}
			return fmt.Errorf("SendMessage: lỗi khi gửi nội dung tin nhắn đến %s (lệnh: '%s', đã gửi %d/%d bytes): %w", c.address.Hex(), message.Command(), totalSent, len(b), errWrite)
		}
		totalSent += n
	}
	return nil
}

// Connect thiết lập kết nối TCP đến node từ xa.
func (c *Connection) Connect() (err error) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.connect && c.tcpConn != nil {
		return nil
	}

	realConnectionAddress, err := c.connectionAddressInternal()
	if err != nil {
		return fmt.Errorf("Connect: lỗi khi lấy địa chỉ kết nối thực cho %s: %w", c.address.Hex(), err)
	}
	tcpConn, err := net.Dial("tcp", realConnectionAddress)
	if err != nil {
		return fmt.Errorf("Connect: lỗi khi quay số đến %s (%s): %w", c.address.Hex(), realConnectionAddress, err)
	}

	c.tcpConn = tcpConn
	if c.requestChan == nil {
		c.requestChan = make(chan network.Request, DefaultRequestChanSize)
	}
	if c.errorChan == nil {
		c.errorChan = make(chan error, DefaultErrorChanSize)
	}
	c.connect = true
	return nil
}

// connectionAddressInternal là phiên bản nội bộ của ConnectionAddress không sử dụng mutex.
func (c *Connection) connectionAddressInternal() (string, error) {
	var err error
	if c.realConnAddr == "" {
		return "", fmt.Errorf("connectionAddressInternal: lỗi khi lấy địa chỉ kết nối thực: %w", err)
	}
	return c.realConnAddr, nil
}

// Disconnect đóng kết nối TCP và các kênh liên quan.
func (c *Connection) Disconnect() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if !c.connect && c.tcpConn == nil && c.requestChan == nil && c.errorChan == nil {
		return nil
	}

	c.connect = false

	var errCloseTcp error
	if c.tcpConn != nil {
		errCloseTcp = c.tcpConn.Close()
		c.tcpConn = nil
	}

	if c.requestChan != nil {
		close(c.requestChan)
		c.requestChan = nil
	}
	if c.errorChan != nil {
		close(c.errorChan)
		c.errorChan = nil
	}

	return errCloseTcp
}

// IsConnect trả về true nếu kết nối đang hoạt động.
func (c *Connection) IsConnect() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.connect && c.tcpConn != nil
}

// ReadRequest đọc liên tục các request từ kết nối TCP.
func (c *Connection) ReadRequest() {
	remoteAddrAtStart := c.RemoteAddrSafe()

	tcpConn := c.getTcpConn()
	if tcpConn == nil {
		c.handleReadError(errors.New("ReadRequest: tcpConn là nil ban đầu"), "kiểm tra tcpConn ban đầu")
		return
	}

	for {
		if !c.IsConnect() {
			c.handleReadError(ErrDisconnected, "kiểm tra trạng thái kết nối trước khi đọc")
			return
		}

		bLength := make([]byte, 8)
		_, err := io.ReadFull(tcpConn, bLength)
		if err != nil {
			c.handleReadError(err, "đọc độ dài tin nhắn")
			return
		}
		messageLength := binary.LittleEndian.Uint64(bLength)

		if messageLength == 0 {
			continue
		}
		if messageLength > MaxMessageLength {
			errMsg := fmt.Sprintf("Tin nhắn từ %s có độ dài %d vượt quá giới hạn %d", remoteAddrAtStart, messageLength, MaxMessageLength)
			c.handleReadError(ErrExceedMessageLength, errMsg)
			return
		}

		data := make([]byte, messageLength)
		bytesRead, err := io.ReadFull(tcpConn, data)
		if err != nil {
			c.handleReadError(err, "đọc nội dung tin nhắn")
			return
		}

		if uint64(bytesRead) != messageLength {
			errMsg := fmt.Sprintf("Đọc thiếu dữ liệu từ %s: mong đợi %d, nhận được %d", remoteAddrAtStart, messageLength, bytesRead)
			c.handleReadError(ErrInvalidMessageLength, errMsg)
			return
		}

		msgProto := &pb.Message{}
		err = proto.Unmarshal(data, msgProto)
		if err != nil {
			errMsg := fmt.Sprintf("Lỗi unmarshal tin nhắn từ %s: %v", remoteAddrAtStart, err)
			c.handleReadError(fmt.Errorf("lỗi unmarshal tin nhắn: %w", err), errMsg)
			return
		}

		req := NewRequest(c, NewMessage(msgProto))

		c.mu.Lock()
		currentRequestChan := c.requestChan
		c.mu.Unlock()

		if currentRequestChan == nil {
			c.handleReadError(errors.New("kênh request là nil"), "kiểm tra kênh request")
			return
		}

		// Thử gửi request vào kênh với timeout
		select {
		case currentRequestChan <- req:
			// Gửi thành công, tiếp tục đọc tin nhắn tiếp theo
		case <-time.After(defaultRequestChanWaitTimeout):
			// Kênh đầy sau khi chờ, báo log và thử lại
			errMsg := fmt.Sprintf("Kênh request đầy cho kết nối %s, không thể gửi request (Command: %s, ID: %s)", remoteAddrAtStart, msgProto.Header.Command, msgProto.Header.ID)
			c.handleReadError(ErrRequestChanFull, errMsg)
			// Thử lại bằng cách quay lại vòng lặp, không thoát
			continue
		}
	}
}

// handleReadError xử lý lỗi và gửi vào errorChan một cách an toàn.
func (c *Connection) handleReadError(err error, context string) {
	var targetError error

	if errors.Is(err, io.EOF) || errors.Is(err, net.ErrClosed) || errors.Is(err, ErrDisconnected) {
		targetError = ErrDisconnected
	} else if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
		targetError = netErr
	} else {
		targetError = err
	}

	c.mu.Lock()
	currentErrorChan := c.errorChan
	c.mu.Unlock()

	if currentErrorChan != nil {
		select {
		case currentErrorChan <- targetError:
		default:
		}
	}
}

func (c *Connection) getTcpConn() net.Conn {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.tcpConn
}

// RemoteAddrSafe trả về địa chỉ từ xa của kết nối TCP một cách an toàn.
func (c *Connection) RemoteAddrSafe() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.remoteAddrSafeInternal()
}

// remoteAddrSafeInternal là phiên bản không khóa của RemoteAddrSafe.
func (c *Connection) remoteAddrSafeInternal() string {
	if c.tcpConn != nil && c.tcpConn.RemoteAddr() != nil {
		return c.tcpConn.RemoteAddr().String()
	}
	if c.realConnAddr != "" {
		return c.realConnAddr
	}
	if c.address != (common.Address{}) {
		return "NodeAddress:" + c.address.Hex()
	}
	return "unknown remote addr"
}

// Clone tạo một bản sao nông của Connection.
func (c *Connection) Clone() network.Connection {
	c.mu.Lock()
	defer c.mu.Unlock()

	addr := c.address
	connType := c.cType
	realAddr := c.realConnAddr

	newConn := NewConnection(
		addr,
		connType,
	)
	if concreteNewConn, ok := newConn.(*Connection); ok {
		concreteNewConn.realConnAddr = realAddr
	}
	return newConn
}

// RemoteAddr trả về địa chỉ từ xa của kết nối TCP dưới dạng chuỗi.
func (c *Connection) RemoteAddr() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.tcpConn == nil {
		if c.realConnAddr != "" {
			return c.realConnAddr + " (not connected via TCP yet)"
		}
		return "nil tcpConn"
	}
	return c.tcpConn.RemoteAddr().String()
}
