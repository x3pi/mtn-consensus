package network

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"time"

	"github.com/ethereum/go-ethereum/common"

	"github.com/meta-node-blockchain/meta-node/pkg/bls"
	p_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/types/network"
)

const (
	// defaultDialTimeout là thời gian chờ mặc định khi kết nối đến parent.
	// defaultRetryParentInterval là khoảng thời gian mặc định giữa các lần thử kết nối lại với parent.
	defaultRetryParentInterval = 5 * time.Second
	// assumedParentConnectionType được sử dụng nếu hằng số từ p_common không có sẵn.
	// TODO: Xác minh và sử dụng hằng số thực tế cho loại kết nối cha từ gói p_common nếu có.
	// Ví dụ: assumedParentConnectionType = p_common.ParentConnectionTypeString
	assumedParentConnectionType = "PARENT"
)

// SocketServer triển khai một máy chủ socket TCP lắng nghe các kết nối đến.
type SocketServer struct {
	connectionsManager network.ConnectionsManager
	listener           net.Listener
	handler            network.Handler

	nodeType string
	version  string
	keyPair  *bls.KeyPair

	ctx        context.Context
	cancelFunc context.CancelFunc

	onConnectedCallBack    []func(connection network.Connection)
	onDisconnectedCallBack []func(connection network.Connection)
}

// NewSocketServer tạo một instance mới của SocketServer.
func NewSocketServer(
	keyPair *bls.KeyPair,
	connectionsManager network.ConnectionsManager,
	handler network.Handler,
	nodeType string,
	version string,
) (network.SocketServer, error) { // Thay đổi: trả về lỗi
	if keyPair == nil {
		return nil, errors.New("NewSocketServer: keyPair không được là nil")
	}
	if connectionsManager == nil {
		return nil, errors.New("NewSocketServer: connectionsManager không được là nil")
	}
	if handler == nil {
		return nil, errors.New("NewSocketServer: handler không được là nil")
	}

	ctx, cancelFunc := context.WithCancel(context.Background())
	s := &SocketServer{
		keyPair:            keyPair,
		connectionsManager: connectionsManager,
		handler:            handler,
		nodeType:           nodeType,
		version:            version,
		ctx:                ctx,
		cancelFunc:         cancelFunc,
	}
	return s, nil
}

// SetContext cho phép đặt một context và cancelFunc bên ngoài cho máy chủ.
func (s *SocketServer) SetContext(ctx context.Context, cancelFunc context.CancelFunc) {
	if ctx == nil || cancelFunc == nil {
		return
	}
	// Nếu server đã có cancelFunc, gọi nó để dọn dẹp context cũ trước khi gán mới.
	if s.cancelFunc != nil {
		s.cancelFunc()
	}
	s.cancelFunc = cancelFunc
	s.ctx = ctx
}

// AddOnConnectedCallBack thêm một hàm callback sẽ được gọi khi một kết nối mới được thiết lập thành công.
func (s *SocketServer) AddOnConnectedCallBack(callBack func(network.Connection)) {
	if callBack == nil {
		return
	}
	s.onConnectedCallBack = append(s.onConnectedCallBack, callBack)
}

// AddOnDisconnectedCallBack thêm một hàm callback sẽ được gọi khi một kết nối bị ngắt.
func (s *SocketServer) AddOnDisconnectedCallBack(callBack func(network.Connection)) {
	if callBack == nil {
		return
	}
	s.onDisconnectedCallBack = append(s.onDisconnectedCallBack, callBack)
}

// Listen bắt đầu lắng nghe các kết nối TCP đến trên địa chỉ được chỉ định.
func (s *SocketServer) Listen(listenAddress string) error {
	if s.listener != nil {
		return errors.New("máy chủ đã lắng nghe trên " + s.listener.Addr().String())
	}

	var err error
	s.listener, err = net.Listen("tcp", listenAddress)
	if err != nil {
		return fmt.Errorf("lỗi khi lắng nghe trên %s: %w", listenAddress, err)
	}

	// Đảm bảo listener được đóng khi hàm Listen kết thúc (do lỗi hoặc do context bị hủy).
	defer func() {
		if listener := s.listener; listener != nil { // Sử dụng biến local để tránh race
			errClose := listener.Close()
			// Chỉ log lỗi nếu không phải là lỗi do listener đã đóng (net.ErrClosed)
			if errClose != nil && !errors.Is(errClose, net.ErrClosed) {
			}
			s.listener = nil // Đặt lại để có thể Listen lại nếu cần
		}
	}()

	for {
		select {
		case <-s.ctx.Done():
			return s.ctx.Err()
		default:
			// Đặt timeout cho Accept để vòng lặp không bị block vô hạn
			if tcpListener, ok := s.listener.(*net.TCPListener); ok {
				// Kiểm tra s.listener có nil không trước khi sử dụng, đề phòng trường hợp Stop() được gọi đồng thời
				currentListener := s.listener
				if currentListener == nil { // Nếu listener đã bị đóng bởi Stop()
					return nil // Thoát vòng lặp một cách an toàn
				}
				// SetDeadline chỉ có trên *net.TCPListener
				if err := tcpListener.SetDeadline(time.Now().Add(1 * time.Second)); err != nil {
					// Nếu listener đã đóng, SetDeadline sẽ báo lỗi.
					if errors.Is(err, net.ErrClosed) {
						// Kiểm tra context một lần nữa để đảm bảo thoát đúng
						if s.ctx.Err() != nil {
							return s.ctx.Err()
						}
						return nil // Listener đã đóng, thoát vòng lặp.
					}
					// Lỗi khác khi đặt deadline không phải là lỗi nghiêm trọng khiến server dừng
					time.Sleep(100 * time.Millisecond) // Chờ một chút trước khi thử lại
					continue
				}
			}

			tcpConn, err := s.listener.Accept()
			if err != nil {
				if opError, ok := err.(net.Error); ok && opError.Timeout() {
					continue // Timeout là bình thường do SetDeadline, tiếp tục vòng lặp để kiểm tra context
				}
				// Nếu lỗi không phải do timeout và context chưa bị hủy, log lỗi.
				if s.ctx.Err() == nil && !errors.Is(err, net.ErrClosed) {
				}
				// Nếu context đã bị hủy, trả về lỗi của context.
				if s.ctx.Err() != nil {
					return s.ctx.Err()
				}
				// Nếu lỗi là do listener đã đóng (ví dụ, do Stop() được gọi), thoát vòng lặp.
				if errors.Is(err, net.ErrClosed) {
					return nil
				}
				continue // Với các lỗi khác, tiếp tục vòng lặp
			}

			conn, errCreation := ConnectionFromTcpConnection(tcpConn)
			if errCreation != nil {
				if errClose := tcpConn.Close(); errClose != nil {
				}
				continue
			}

			go s.OnConnect(conn)        // Xử lý handshake và thêm vào manager
			go s.HandleConnection(conn) // Xử lý đọc/ghi cho kết nối
		}
	}
}

// Stop dừng máy chủ lắng nghe và đóng tất cả các kết nối đang hoạt động.
func (s *SocketServer) Stop() {

	// 1. Hủy context để báo hiệu các goroutine dừng lại.
	s.cancelFunc()

	// 2. Đóng listener một cách rõ ràng.
	// Sử dụng biến local để tránh race condition nếu s.listener có thể được gán lại.
	listener := s.listener
	if listener != nil {
		if err := listener.Close(); err != nil && !errors.Is(err, net.ErrClosed) {
		}
		s.listener = nil // Đặt lại để có thể Listen() lại nếu cần.
	} else {
	}

	// 3. Cân nhắc đóng tất cả các kết nối đang hoạt động một cách chủ động.
	// HandleConnection đã có defer conn.Disconnect(), nhưng việc gọi ở đây có thể tăng tốc quá trình đóng.
	// Tuy nhiên, điều này cần cẩn thận để tránh race với HandleConnection.
	// Hiện tại, dựa vào việc HandleConnection sẽ thoát khi context bị hủy.

}

// OnConnect xử lý các tác vụ cần thiết khi một kết nối mới được thiết lập (thường là kết nối đến).
func (s *SocketServer) OnConnect(conn network.Connection) {
	if conn == nil {
		return
	}
	var addressForInitMsgBytes []byte

	if s.connectionsManager != nil {
		parentConn := s.connectionsManager.ParentConnection()
		if parentConn != nil && parentConn.Address() != (common.Address{}) {
			addressForInitMsgBytes = parentConn.Address().Bytes()
		}
	}

	if len(addressForInitMsgBytes) == 0 {
		if s.keyPair != nil { // keyPair đã được kiểm tra not nil trong NewSocketServer
			addressForInitMsgBytes = s.keyPair.Address().Bytes()
		} else {
			// Điều này không nên xảy ra nếu NewSocketServer trả về lỗi khi keyPair nil.
			conn.Disconnect()    // Ngắt kết nối nếu không thể gửi INIT đúng cách
			s.OnDisconnect(conn) // Gọi callback OnDisconnect thủ công
			return
		}
	}
	initMsg := &pb.InitConnection{
		Address: addressForInitMsgBytes, // Địa chỉ của node này (hoặc parent của nó)
		Type:    s.nodeType,             // Loại của node này
		Replace: true,
	}
	err := SendMessage(conn, p_common.InitConnection, initMsg, s.version)
	if err != nil {
		conn.Disconnect() // Ngắt kết nối nếu INIT thất bại
		// s.OnDisconnect(conn) // OnDisconnect sẽ được gọi từ HandleConnection khi kết nối đóng
		return
	}

	// Hiện tại, giả sử callback sẽ xử lý việc thêm vào ConnectionsManager nếu cần.
	for _, callBack := range s.onConnectedCallBack {
		if callBack != nil {
			callBack(conn)
		}
	}
}

// OnDisconnect xử lý các tác vụ cần thiết khi một kết nối bị ngắt.
func (s *SocketServer) OnDisconnect(conn network.Connection) {
	if conn == nil {
		return
	}

	// Log thông tin trước khi xóa khỏi manager
	logger.Error(fmt.Sprintf("Connection disconnected: %s", conn.String()))

	if s.connectionsManager != nil { // connectionsManager đã được kiểm tra not nil trong NewSocketServer
		s.connectionsManager.RemoveConnection(conn)
	}

	// Gọi các callback onDisconnected
	for _, callBack := range s.onDisconnectedCallBack {
		if callBack != nil {
			callBack(conn)
		}
	}
}

// HandleConnection quản lý vòng đời của một kết nối đơn lẻ.
func (s *SocketServer) HandleConnection(conn network.Connection) error {
	if conn == nil {
		errMsg := "HandleConnection: được gọi với kết nối nil."
		return errors.New(errMsg)
	}

	// connIdentifier được sử dụng để logging, giúp dễ dàng theo dõi một kết nối cụ thể.
	// Nó bao gồm địa chỉ từ xa, địa chỉ node (nếu đã init), loại kết nối và ID của tin nhắn cuối cùng.
	var connIdentifier string
	updateConnIdentifier := func(lastMsgID string) {
		nodeAddr := "N/A"
		// Sử dụng conn.Address() để lấy địa chỉ node
		if conn.Address() != (common.Address{}) {
			nodeAddr = conn.Address().Hex()
		}
		connType := "N/A"
		// Sử dụng conn.Type() để lấy loại kết nối
		if conn.Type() != "" {
			connType = conn.Type()
		}
		// Sử dụng conn.RemoteAddrSafe() để lấy địa chỉ từ xa an toàn
		connIdentifier = fmt.Sprintf("Remote:%s (Node:%s, Type:%s, LastMsgID:%s)",
			conn.RemoteAddrSafe(),
			nodeAddr,
			connType,
			lastMsgID,
		)
	}
	updateConnIdentifier("N/A_Initial") // Giá trị ban đầu

	// Lấy kênh request và error từ kết nối
	requestChan, errorChan := conn.RequestChan()
	if requestChan == nil || errorChan == nil {
		errMsg := fmt.Sprintf("HandleConnection: Kênh request hoặc error từ kết nối %s là nil. Không thể xử lý.", connIdentifier)
		// Không Disconnect ở đây vì có thể conn đã ở trạng thái không tốt, defer sẽ xử lý.
		return errors.New(errMsg)
	}

	// Khởi chạy goroutine đọc request từ kết nối.
	// ReadRequest sẽ gửi request vào requestChan và lỗi vào errorChan.
	go conn.ReadRequest()

	// Defer Disconnect và OnDisconnect để đảm bảo dọn dẹp khi HandleConnection kết thúc,
	// bất kể lý do thoát là gì (lỗi, context hủy, kênh đóng).
	defer func() {
		// Ngắt kết nối. Lỗi được log bên trong Disconnect nếu có.
		// Các lỗi như net.ErrClosed hay ErrDisconnected là bình thường khi đóng.
		// Sử dụng conn.Disconnect() để ngắt kết nối
		// Giả sử ErrDisconnected được định nghĩa trong package này hoặc được import đúng cách
		if err := conn.Disconnect(); err != nil && !errors.Is(err, net.ErrClosed) && !errors.Is(err, ErrDisconnected) {
		}
		s.OnDisconnect(conn) // Gọi OnDisconnect sau khi đã ngắt kết nối.
	}()

	for {

		select {
		// Kiểm tra context của server để dừng graceful
		case <-s.ctx.Done():
			return s.ctx.Err()

		case request, ok := <-requestChan:
			if !ok { // Kênh request đã đóng (thường do ReadRequest kết thúc)
				return errors.New("kênh request đã đóng") // Đây là một lối thoát bình thường.
			}
			if request == nil { // Nhận được request nil
				continue
			}

			// Cập nhật connIdentifier với ID và Command của tin nhắn mới nhất để log dễ theo dõi hơn.
			// Sử dụng request.Message() để lấy tin nhắn
			// Sử dụng message.ID() và message.Command()
			if request.Message() != nil {
				msgID := request.Message().ID()
				if msgID == "" {
					msgID = "N/A_NoMsgID"
				}
				updateConnIdentifier(msgID)
			} else {
				updateConnIdentifier("N/A_NilMessage")
			}

			// Xử lý request trong một goroutine mới để không block vòng lặp select.
			// Điều này cho phép HandleConnection tiếp tục nhận các request khác
			// hoặc tín hiệu từ s.ctx hay errorChan.
			go func(req network.Request) {
				// s.handler đã được kiểm tra not nil trong NewSocketServer
				// Sử dụng s.handler.HandleRequest để xử lý request
				if errHandler := s.handler.HandleRequest(req); errHandler != nil {
				} else {
				}
			}(request)

		case err, ok := <-errorChan:
			if !ok { // Kênh error đã đóng
				return errors.New("kênh error đã đóng")
			}
			// Lỗi từ kết nối (ví dụ: từ ReadRequest như io.EOF, net.ErrClosed, hoặc lỗi unmarshal).
			// ErrDisconnected là lỗi chuẩn hóa mà ReadRequest gửi vào errorChan khi kết nối đóng.
			if errors.Is(err, ErrDisconnected) || errors.Is(err, io.EOF) || errors.Is(err, net.ErrClosed) {
			} else if err != nil { // Các lỗi khác từ ReadRequest
			} else {
				// err == nil, nhưng ok == true (hiếm khi xảy ra với error channels, nhưng có thể nếu ai đó gửi nil error)
				// Trả về một lỗi chung để đảm bảo defer được thực thi đúng cách
				return errors.New("nil error received on errorChan")
			}
			return err // Trả về lỗi để kết thúc HandleConnection, defer sẽ chạy Disconnect và OnDisconnect.
		}
	}
}

// SetKeyPair cập nhật cặp khóa BLS cho máy chủ.
func (s *SocketServer) SetKeyPair(newKeyPair *bls.KeyPair) {
	if newKeyPair == nil {
		return
	}
	s.keyPair = newKeyPair
}

func (s *SocketServer) RetryConnectToParent(disconnectedParentConn network.Connection) { // Đổi tên tham số để rõ ràng hơn
	if s.connectionsManager == nil {
		return
	}

	// Kiểm tra cơ bản đối với kết nối cha đã bị ngắt
	if disconnectedParentConn == nil {
		return
	}

	// Đảm bảo rằng kết nối được cung cấp thực sự là loại PARENT như một kiểm tra an toàn.
	// Logic gọi hàm này nên đảm bảo điều này.
	// Sử dụng hằng số assumedParentConnectionType đã định nghĩa trong file.
	if disconnectedParentConn.Type() != assumedParentConnectionType { //
		return
	}

	// Clone đối tượng kết nối đã được truyền vào, vì nó đại diện cho parent mà chúng ta muốn thử lại.
	// Phương thức Clone sẽ tạo một đối tượng Connection mới với các kênh mới và trạng thái "sạch".
	clonedParentConn := disconnectedParentConn.Clone()
	if clonedParentConn == nil { // Clone() không nên trả về nil dựa trên mã nguồn hiện tại, nhưng kiểm tra vẫn tốt.
		return
	}

	go func(connToRetry network.Connection) {
		for {
			// Kiểm tra context của server trước mỗi lần thử.
			// Nếu server đang dừng, không thử kết nối lại nữa.
			select {
			case <-s.ctx.Done():
				return
			default:
				// Context chưa hủy, tiếp tục.
			}

			// Phương thức Connect sẽ thiết lập kết nối TCP mới và tạo lại các kênh nếu cần.
			err := connToRetry.Connect()
			if err == nil {
				if s.connectionsManager != nil {
					// Quan trọng: Kết nối connToRetry (bản clone) mới, đã kết nối thành công,
					// trở thành kết nối cha mới.
					s.connectionsManager.AddParentConnection(connToRetry) //
				}
				s.OnConnect(connToRetry)           // Xử lý handshake (gửi INIT) trên kết nối mới.
				go s.HandleConnection(connToRetry) // Bắt đầu xử lý kết nối mới này.
				return                             // Kết nối thành công, thoát goroutine thử lại
			}

			// Chờ trước khi thử lại, nhưng cũng kiểm tra context server.
			select {
			case <-time.After(defaultRetryParentInterval): //
				// Tiếp tục vòng lặp để thử lại.
			case <-s.ctx.Done():

				return
			}
		}
	}(clonedParentConn)
}

// Bạn cũng nên áp dụng logic tương tự cho hàm StopAndRetryConnectToParent nếu nó được sử dụng.
// Hiện tại, logic thử lại trong StopAndRetryConnectToParent gần như giống hệt RetryConnectToParent.
func (s *SocketServer) StopAndRetryConnectToParent(disconnectedParentConn network.Connection) {
	if s.connectionsManager == nil {
		return
	}

	if disconnectedParentConn == nil {
		return
	}

	if disconnectedParentConn.Type() != assumedParentConnectionType { //

		return
	}

	// KHÔNG gọi s.Stop() ở đây theo như ghi chú trong code gốc.

	clonedParentConn := disconnectedParentConn.Clone() //
	if clonedParentConn == nil {
		return
	}

	go func(connToRetry network.Connection) {
		for {
			select {
			case <-s.ctx.Done():
				return
			default:
			}

			err := connToRetry.Connect() //
			if err == nil {
				if s.connectionsManager != nil {
					s.connectionsManager.AddParentConnection(connToRetry) //
				}
				s.OnConnect(connToRetry)           //
				go s.HandleConnection(connToRetry) //
				return
			}

			select {
			case <-time.After(defaultRetryParentInterval): //
			case <-s.ctx.Done():
				return
			}
		}
	}(clonedParentConn)
}
