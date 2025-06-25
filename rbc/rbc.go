package rbc

import (
	"encoding/binary"
	"fmt"
	"io"
	"log"
	"net"
	"sync"
	"time"

	"google.golang.org/protobuf/proto"
)

// broadcastState theo dõi trạng thái của một lần broadcast cụ thể.
type broadcastState struct {
	mu         sync.Mutex
	echoRecvd  map[int32]bool
	readyRecvd map[int32]bool
	sentEcho   bool
	sentReady  bool
	delivered  bool
	payload    []byte
}

// Process đại diện cho một node trong hệ thống RBC.
type Process struct {
	ID        int32
	Peers     map[int32]string // Map từ ID của peer đến địa chỉ mạng
	N         int
	F         int
	Listener  net.Listener
	Delivered chan []byte // Kênh để gửi dữ liệu đã được giao cho ứng dụng

	// logs lưu trữ trạng thái của mỗi lần broadcast
	logs   map[string]*broadcastState
	logsMu sync.Mutex
}

// NewProcess tạo một Process mới.
func NewProcess(id int32, peers map[int32]string) *Process {
	n := len(peers)
	f := (n - 1) / 3
	if n <= 3*f {
		log.Fatalf("Hệ thống không thể chịu lỗi với n=%d, f=%d. Yêu cầu n > 3f.", n, f)
	}

	return &Process{
		ID:        id,
		Peers:     peers,
		N:         n,
		F:         f,
		Delivered: make(chan []byte, 1024),
		logs:      make(map[string]*broadcastState),
	}
}

// Bắt đầu lắng nghe các kết nối đến.
func (p *Process) Start() {
	addr := p.Peers[p.ID]
	var err error
	p.Listener, err = net.Listen("tcp", addr)
	if err != nil {
		log.Fatalf("Node %d không thể lắng nghe trên %s: %v", p.ID, addr, err)
	}
	log.Printf("Node %d đang lắng nghe trên %s", p.ID, addr)

	for {
		conn, err := p.Listener.Accept()
		if err != nil {
			log.Printf("Node %d lỗi khi chấp nhận kết nối: %v", p.ID, err)
			continue
		}
		go p.handleConnection(conn)
	}
}

// Xử lý một kết nối đến từ một peer khác.
func (p *Process) handleConnection(conn net.Conn) {
	defer conn.Close()
	for {
		// Đọc độ dài của thông điệp (4 bytes)
		lenBuf := make([]byte, 4)
		_, err := io.ReadFull(conn, lenBuf)
		if err != nil {
			if err != io.EOF {
				// log.Printf("Lỗi đọc độ dài thông điệp: %v", err)
			}
			return
		}
		length := binary.BigEndian.Uint32(lenBuf)

		// Đọc dữ liệu thông điệp
		msgBuf := make([]byte, length)
		_, err = io.ReadFull(conn, msgBuf)
		if err != nil {
			// log.Printf("Lỗi đọc nội dung thông điệp: %v", err)
			return
		}

		var msg RBCMessage
		if err := proto.Unmarshal(msgBuf, &msg); err != nil {
			log.Printf("Lỗi giải mã protobuf: %v", err)
			continue
		}

		// Xử lý thông điệp trong một goroutine riêng để không chặn vòng lặp nhận
		go p.handleMessage(&msg)
	}
}

// Gửi một thông điệp đến một peer cụ thể.
func (p *Process) send(targetID int32, msg *RBCMessage) {
	addr, ok := p.Peers[targetID]
	if !ok {
		log.Printf("Không tìm thấy địa chỉ cho peer %d", targetID)
		return
	}

	conn, err := net.Dial("tcp", addr)
	if err != nil {
		// log.Printf("Node %d không thể kết nối đến %d tại %s: %v", p.ID, targetID, addr, err)
		return
	}
	defer conn.Close()

	data, err := proto.Marshal(msg)
	if err != nil {
		log.Printf("Lỗi mã hóa protobuf: %v", err)
		return
	}

	// Gửi độ dài thông điệp trước
	lenBuf := make([]byte, 4)
	binary.BigEndian.PutUint32(lenBuf, uint32(len(data)))
	if _, err := conn.Write(lenBuf); err != nil {
		// log.Printf("Lỗi gửi độ dài thông điệp tới %d: %v", targetID, err)
		return
	}

	// Gửi nội dung thông điệp
	if _, err := conn.Write(data); err != nil {
		// log.Printf("Lỗi gửi nội dung thông điệp tới %d: %v", targetID, err)
	}
}

// broadcast gửi một thông điệp đến tất cả các peer.
func (p *Process) broadcast(msg *RBCMessage) {
	msg.NetworkSenderId = p.ID // Đặt ID của người gửi mạng là chính nó
	for id := range p.Peers {
		if id == p.ID {
			// Xử lý thông điệp cho chính mình cục bộ
			go p.handleMessage(msg)
		} else {
			go p.send(id, msg)
		}
	}
}

// getOrCreateState lấy hoặc tạo trạng thái cho một lần broadcast.
func (p *Process) getOrCreateState(key string, payload []byte) *broadcastState {
	p.logsMu.Lock()
	defer p.logsMu.Unlock()

	state, exists := p.logs[key]
	if !exists {
		state = &broadcastState{
			echoRecvd:  make(map[int32]bool),
			readyRecvd: make(map[int32]bool),
			payload:    payload,
		}
		p.logs[key] = state
	}
	return state
}

// handleMessage là nơi logic chính của RBC được thực thi.
func (p *Process) handleMessage(msg *RBCMessage) {
	key := fmt.Sprintf("%d-%s", msg.OriginalSenderId, msg.MessageId)
	state := p.getOrCreateState(key, msg.Payload)

	state.mu.Lock()
	defer state.mu.Unlock()

	switch msg.Type {
	case MessageType_INIT:
		if !state.sentEcho {
			state.sentEcho = true
			log.Printf("Node %d nhận INIT, gửi ECHO cho message %s", p.ID, key)
			echoMsg := &RBCMessage{
				Type:             MessageType_ECHO,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
			}
			p.broadcast(echoMsg)
		}

	case MessageType_ECHO:
		state.echoRecvd[msg.NetworkSenderId] = true
		// Ngưỡng để gửi READY: (N+F)/2 + 1 trong các thuật toán Byzantine,
		// hoặc n-f trong crash-stop. (N+F)/2 + 1 cũng hoạt động cho crash-stop.
		// Chúng ta dùng một ngưỡng mạnh: (N + F) / 2
		if len(state.echoRecvd) > (p.N+p.F)/2 && !state.sentReady {
			state.sentReady = true
			log.Printf("Node %d nhận đủ ECHO, gửi READY cho message %s", p.ID, key)
			readyMsg := &RBCMessage{
				Type:             MessageType_READY,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
			}
			p.broadcast(readyMsg)
		}

	case MessageType_READY:
		state.readyRecvd[msg.NetworkSenderId] = true

		// Khuếch đại READY: nếu nhận đủ f+1 READY, tự mình gửi READY để giúp các node chậm hơn.
		if len(state.readyRecvd) > p.F && !state.sentReady {
			state.sentReady = true
			log.Printf("Node %d nhận f+1 READY, khuếch đại READY cho message %s", p.ID, key)
			readyMsg := &RBCMessage{
				Type:             MessageType_READY,
				OriginalSenderId: msg.OriginalSenderId,
				MessageId:        msg.MessageId,
				Payload:          msg.Payload,
			}
			p.broadcast(readyMsg)
		}

		// Giao nhận: nếu nhận đủ 2f+1 READY, giao thông điệp.
		if len(state.readyRecvd) > 2*p.F && !state.delivered {
			state.delivered = true
			log.Printf("Node %d đã GIAO (DELIVER) message %s: %s", p.ID, key, string(state.payload))
			p.Delivered <- state.payload
		}
	}
}

// StartBroadcast được ứng dụng gọi để bắt đầu một lần broadcast mới.
func (p *Process) StartBroadcast(payload []byte) {
	messageID := fmt.Sprintf("%d-%d", p.ID,
		// Sử dụng nano giây để có ID gần như duy nhất
		time.Now().UnixNano(),
	)
	log.Printf("Node %d bắt đầu broadcast message %s", p.ID, messageID)
	initMsg := &RBCMessage{
		Type:             MessageType_INIT,
		OriginalSenderId: p.ID,
		MessageId:        messageID,
		Payload:          payload,
	}
	p.broadcast(initMsg)
}
