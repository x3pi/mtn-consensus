package network

import (
	"encoding/hex"
	"errors"
	"fmt"

	"github.com/ethereum/go-ethereum/common"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"

	cm "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/types/network"
)

// Message đại diện cho một tin nhắn được gửi qua mạng.
// Nó đóng gói một Protocol Buffer message (pb.Message) và cung cấp các phương thức tiện ích.
type Message struct {
	proto *pb.Message // proto là con trỏ đến đối tượng Protocol Buffer message cơ bản.
}

// NewMessage tạo một instance mới của network.Message từ một pb.Message.
// pbMessage: Con trỏ đến Protocol Buffer message.
func NewMessage(pbMessage *pb.Message) network.Message {
	if pbMessage == nil {
		logger.Warn("NewMessage: pbMessage đầu vào là nil. Một Message với proto và header rỗng sẽ được tạo.")
		// Khởi tạo một pb.Message rỗng với Header để tránh panic ở các hàm accessor
		pbMessage = &pb.Message{
			Header: &pb.Header{},
		}
	} else if pbMessage.Header == nil {
		logger.Warn("NewMessage: pbMessage.Header là nil. Sẽ khởi tạo một Header rỗng.")
		pbMessage.Header = &pb.Header{}
	}
	return &Message{
		proto: pbMessage,
	}
}

// Marshal chuyển đổi Message thành một mảng byte sử dụng Protocol Buffers.
func (m *Message) Marshal() ([]byte, error) {
	if m == nil || m.proto == nil {
		// Với NewMessage đã được cập nhật, m.proto không nên nil nếu m không nil.
		// Tuy nhiên, kiểm tra này vẫn giữ an toàn.
		return nil, errors.New("Message.Marshal: không thể marshal tin nhắn nil hoặc tin nhắn với proto nil")
	}
	return proto.Marshal(m.proto)
}

// Unmarshal giải mã (unmarshal) phần body của Message vào một cấu trúc Protocol Buffer được cung cấp.
// protoStruct: Con trỏ đến cấu trúc Protocol Buffer mà body sẽ được giải mã vào.
func (m *Message) Unmarshal(protoStruct protoreflect.ProtoMessage) error {
	if m == nil || m.proto == nil {
		return errors.New("Message.Unmarshal: không thể unmarshal từ tin nhắn nil hoặc tin nhắn với proto nil")
	}
	if protoStruct == nil {
		return errors.New("Message.Unmarshal: protoStruct không được là nil")
	}
	if m.proto.Body == nil {
		// Nếu body là nil, proto.Unmarshal thường sẽ không làm gì cả hoặc trả về lỗi nếu protoStruct yêu cầu dữ liệu.
		// Điều này thường là hành vi mong muốn (ví dụ: một tin nhắn không có body sẽ giải mã thành một struct rỗng).
		logger.Debug("Message.Unmarshal: Body của tin nhắn (ID: %s, Command: %s) là nil. Kết quả unmarshal sẽ phụ thuộc vào proto.Unmarshal và protoStruct.", m.ID(), m.Command())
		// Để tránh lỗi "invalid wire-format data" nếu body là nil và protoStruct không rỗng,
		// chúng ta có thể trả về nil ở đây nếu protoStruct có thể là một tin nhắn rỗng hợp lệ.
		// Hoặc, nếu protoStruct luôn yêu cầu body, thì để proto.Unmarshal xử lý và trả lỗi.
		// Giữ nguyên hành vi hiện tại: để proto.Unmarshal quyết định.
	}
	return proto.Unmarshal(m.proto.Body, protoStruct)
}

// String trả về một chuỗi biểu diễn của Message, hữu ích cho việc logging và debugging.
func (m *Message) String() string {
	if m == nil {
		return "<Message: (Message) nil>"
	}
	if m.proto == nil {
		// Điều này không nên xảy ra nếu NewMessage hoạt động đúng
		return "<Message: (proto) nil>"
	}
	// m.proto.Header không nên nil do NewMessage đã xử lý
	if m.proto.Header == nil {
		// Vẫn giữ kiểm tra này như một biện pháp phòng ngừa cuối cùng
		logger.Error("Message.String: m.proto.Header là nil mặc dù đã có kiểm tra trong NewMessage.")
		return fmt.Sprintf(`
	Header: <nil panic guard>
	Body (hex): %s
`, hex.EncodeToString(m.proto.Body))
	}

	return fmt.Sprintf(`
	Message Info:
		ID: %s
		Command: %s
		Version: %s
		ToAddress: %s
		Pubkey: %s
		Sign: %s
	Body (hex): %s
`,
		m.proto.Header.ID,
		m.proto.Header.Command,
		m.proto.Header.Version,
		common.BytesToAddress(m.proto.Header.ToAddress).Hex(),
		hex.EncodeToString(m.proto.Header.Pubkey),
		hex.EncodeToString(m.proto.Header.Sign),
		hex.EncodeToString(m.proto.Body),
	)
}

// Command trả về lệnh của tin nhắn từ header.
// Trả về chuỗi rỗng nếu message, proto hoặc header là nil (đã được bảo vệ bởi NewMessage).
func (m *Message) Command() string {
	if m == nil || m.proto == nil || m.proto.Header == nil { // Kiểm tra phòng ngừa
		return ""
	}
	return m.proto.Header.Command
}

// Body trả về phần body (dữ liệu thô) của tin nhắn.
// Trả về nil nếu message hoặc proto là nil (đã được bảo vệ bởi NewMessage).
func (m *Message) Body() []byte {
	if m == nil || m.proto == nil { // Kiểm tra phòng ngừa
		return nil
	}
	return m.proto.Body
}

// Pubkey trả về khóa công khai (public key) từ header của tin nhắn.
// Chuyển đổi mảng byte thành kiểu cm.PublicKey.
// Trả về cm.PublicKey rỗng nếu có vấn đề (đã được bảo vệ bởi NewMessage).
func (m *Message) Pubkey() cm.PublicKey {
	if m == nil || m.proto == nil || m.proto.Header == nil || m.proto.Header.Pubkey == nil { // Kiểm tra phòng ngừa
		return cm.PublicKey{}
	}
	return cm.PubkeyFromBytes(m.proto.Header.Pubkey)
}

// Sign trả về chữ ký từ header của tin nhắn.
// Chuyển đổi mảng byte thành kiểu cm.Sign.
// Trả về cm.Sign rỗng nếu có vấn đề (đã được bảo vệ bởi NewMessage).
func (m *Message) Sign() cm.Sign {
	if m == nil || m.proto == nil || m.proto.Header == nil || m.proto.Header.Sign == nil { // Kiểm tra phòng ngừa
		return cm.Sign{}
	}
	return cm.SignFromBytes(m.proto.Header.Sign)
}

// ToAddress trả về địa chỉ người nhận từ header của tin nhắn.
// Chuyển đổi mảng byte thành kiểu common.Address.
// Trả về common.Address rỗng (zero address) nếu có vấn đề (đã được bảo vệ bởi NewMessage).
func (m *Message) ToAddress() common.Address {
	if m == nil || m.proto == nil || m.proto.Header == nil || m.proto.Header.ToAddress == nil { // Kiểm tra phòng ngừa
		return common.Address{}
	}
	return common.BytesToAddress(m.proto.Header.ToAddress)
}

// ID trả về ID duy nhất của tin nhắn từ header.
// Trả về chuỗi rỗng nếu message, proto hoặc header là nil (đã được bảo vệ bởi NewMessage).
func (m *Message) ID() string {
	if m == nil || m.proto == nil || m.proto.Header == nil { // Kiểm tra phòng ngừa
		return ""
	}
	return m.proto.Header.ID
}
