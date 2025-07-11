// Code generated by protoc-gen-go. DO NOT EDIT.
// versions:
// 	protoc-gen-go v1.25.0-devel
// 	protoc        v3.20.3
// source: bba.proto

package mtn_proto

import (
	protoreflect "google.golang.org/protobuf/reflect/protoreflect"
	protoimpl "google.golang.org/protobuf/runtime/protoimpl"
	reflect "reflect"
	sync "sync"
)

const (
	// Verify that this generated code is sufficiently up-to-date.
	_ = protoimpl.EnforceVersion(20 - protoimpl.MinVersion)
	// Verify that runtime/protoimpl is sufficiently up-to-date.
	_ = protoimpl.EnforceVersion(protoimpl.MaxVersion - 20)
)

// Tin nhắn chung cho BBA
type BBAMessage struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	SessionId string `protobuf:"bytes,1,opt,name=session_id,json=sessionId,proto3" json:"session_id,omitempty"`
	Epoch     int32  `protobuf:"varint,2,opt,name=epoch,proto3" json:"epoch,omitempty"`
	SenderId  int32  `protobuf:"varint,3,opt,name=sender_id,json=senderId,proto3" json:"sender_id,omitempty"`
	// Types that are assignable to Content:
	//	*BBAMessage_BvalRequest
	//	*BBAMessage_AuxRequest
	Content isBBAMessage_Content `protobuf_oneof:"content"`
}

func (x *BBAMessage) Reset() {
	*x = BBAMessage{}
	if protoimpl.UnsafeEnabled {
		mi := &file_bba_proto_msgTypes[0]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *BBAMessage) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*BBAMessage) ProtoMessage() {}

func (x *BBAMessage) ProtoReflect() protoreflect.Message {
	mi := &file_bba_proto_msgTypes[0]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use BBAMessage.ProtoReflect.Descriptor instead.
func (*BBAMessage) Descriptor() ([]byte, []int) {
	return file_bba_proto_rawDescGZIP(), []int{0}
}

func (x *BBAMessage) GetSessionId() string {
	if x != nil {
		return x.SessionId
	}
	return ""
}

func (x *BBAMessage) GetEpoch() int32 {
	if x != nil {
		return x.Epoch
	}
	return 0
}

func (x *BBAMessage) GetSenderId() int32 {
	if x != nil {
		return x.SenderId
	}
	return 0
}

func (m *BBAMessage) GetContent() isBBAMessage_Content {
	if m != nil {
		return m.Content
	}
	return nil
}

func (x *BBAMessage) GetBvalRequest() *BvalRequest {
	if x, ok := x.GetContent().(*BBAMessage_BvalRequest); ok {
		return x.BvalRequest
	}
	return nil
}

func (x *BBAMessage) GetAuxRequest() *AuxRequest {
	if x, ok := x.GetContent().(*BBAMessage_AuxRequest); ok {
		return x.AuxRequest
	}
	return nil
}

type isBBAMessage_Content interface {
	isBBAMessage_Content()
}

type BBAMessage_BvalRequest struct {
	BvalRequest *BvalRequest `protobuf:"bytes,4,opt,name=bval_request,json=bvalRequest,proto3,oneof"`
}

type BBAMessage_AuxRequest struct {
	AuxRequest *AuxRequest `protobuf:"bytes,5,opt,name=aux_request,json=auxRequest,proto3,oneof"`
}

func (*BBAMessage_BvalRequest) isBBAMessage_Content() {}

func (*BBAMessage_AuxRequest) isBBAMessage_Content() {}

// Yêu cầu BVAL(v)
type BvalRequest struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	Value bool `protobuf:"varint,1,opt,name=value,proto3" json:"value,omitempty"`
}

func (x *BvalRequest) Reset() {
	*x = BvalRequest{}
	if protoimpl.UnsafeEnabled {
		mi := &file_bba_proto_msgTypes[1]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *BvalRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*BvalRequest) ProtoMessage() {}

func (x *BvalRequest) ProtoReflect() protoreflect.Message {
	mi := &file_bba_proto_msgTypes[1]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use BvalRequest.ProtoReflect.Descriptor instead.
func (*BvalRequest) Descriptor() ([]byte, []int) {
	return file_bba_proto_rawDescGZIP(), []int{1}
}

func (x *BvalRequest) GetValue() bool {
	if x != nil {
		return x.Value
	}
	return false
}

// Yêu cầu AUX(v)
type AuxRequest struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	Value bool `protobuf:"varint,1,opt,name=value,proto3" json:"value,omitempty"`
}

func (x *AuxRequest) Reset() {
	*x = AuxRequest{}
	if protoimpl.UnsafeEnabled {
		mi := &file_bba_proto_msgTypes[2]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *AuxRequest) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*AuxRequest) ProtoMessage() {}

func (x *AuxRequest) ProtoReflect() protoreflect.Message {
	mi := &file_bba_proto_msgTypes[2]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use AuxRequest.ProtoReflect.Descriptor instead.
func (*AuxRequest) Descriptor() ([]byte, []int) {
	return file_bba_proto_rawDescGZIP(), []int{2}
}

func (x *AuxRequest) GetValue() bool {
	if x != nil {
		return x.Value
	}
	return false
}

var File_bba_proto protoreflect.FileDescriptor

var file_bba_proto_rawDesc = []byte{
	0x0a, 0x09, 0x62, 0x62, 0x61, 0x2e, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x12, 0x09, 0x6d, 0x74, 0x6e,
	0x5f, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x22, 0xe0, 0x01, 0x0a, 0x0a, 0x42, 0x42, 0x41, 0x4d, 0x65,
	0x73, 0x73, 0x61, 0x67, 0x65, 0x12, 0x1d, 0x0a, 0x0a, 0x73, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e,
	0x5f, 0x69, 0x64, 0x18, 0x01, 0x20, 0x01, 0x28, 0x09, 0x52, 0x09, 0x73, 0x65, 0x73, 0x73, 0x69,
	0x6f, 0x6e, 0x49, 0x64, 0x12, 0x14, 0x0a, 0x05, 0x65, 0x70, 0x6f, 0x63, 0x68, 0x18, 0x02, 0x20,
	0x01, 0x28, 0x05, 0x52, 0x05, 0x65, 0x70, 0x6f, 0x63, 0x68, 0x12, 0x1b, 0x0a, 0x09, 0x73, 0x65,
	0x6e, 0x64, 0x65, 0x72, 0x5f, 0x69, 0x64, 0x18, 0x03, 0x20, 0x01, 0x28, 0x05, 0x52, 0x08, 0x73,
	0x65, 0x6e, 0x64, 0x65, 0x72, 0x49, 0x64, 0x12, 0x3b, 0x0a, 0x0c, 0x62, 0x76, 0x61, 0x6c, 0x5f,
	0x72, 0x65, 0x71, 0x75, 0x65, 0x73, 0x74, 0x18, 0x04, 0x20, 0x01, 0x28, 0x0b, 0x32, 0x16, 0x2e,
	0x6d, 0x74, 0x6e, 0x5f, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x2e, 0x42, 0x76, 0x61, 0x6c, 0x52, 0x65,
	0x71, 0x75, 0x65, 0x73, 0x74, 0x48, 0x00, 0x52, 0x0b, 0x62, 0x76, 0x61, 0x6c, 0x52, 0x65, 0x71,
	0x75, 0x65, 0x73, 0x74, 0x12, 0x38, 0x0a, 0x0b, 0x61, 0x75, 0x78, 0x5f, 0x72, 0x65, 0x71, 0x75,
	0x65, 0x73, 0x74, 0x18, 0x05, 0x20, 0x01, 0x28, 0x0b, 0x32, 0x15, 0x2e, 0x6d, 0x74, 0x6e, 0x5f,
	0x70, 0x72, 0x6f, 0x74, 0x6f, 0x2e, 0x41, 0x75, 0x78, 0x52, 0x65, 0x71, 0x75, 0x65, 0x73, 0x74,
	0x48, 0x00, 0x52, 0x0a, 0x61, 0x75, 0x78, 0x52, 0x65, 0x71, 0x75, 0x65, 0x73, 0x74, 0x42, 0x09,
	0x0a, 0x07, 0x63, 0x6f, 0x6e, 0x74, 0x65, 0x6e, 0x74, 0x22, 0x23, 0x0a, 0x0b, 0x42, 0x76, 0x61,
	0x6c, 0x52, 0x65, 0x71, 0x75, 0x65, 0x73, 0x74, 0x12, 0x14, 0x0a, 0x05, 0x76, 0x61, 0x6c, 0x75,
	0x65, 0x18, 0x01, 0x20, 0x01, 0x28, 0x08, 0x52, 0x05, 0x76, 0x61, 0x6c, 0x75, 0x65, 0x22, 0x22,
	0x0a, 0x0a, 0x41, 0x75, 0x78, 0x52, 0x65, 0x71, 0x75, 0x65, 0x73, 0x74, 0x12, 0x14, 0x0a, 0x05,
	0x76, 0x61, 0x6c, 0x75, 0x65, 0x18, 0x01, 0x20, 0x01, 0x28, 0x08, 0x52, 0x05, 0x76, 0x61, 0x6c,
	0x75, 0x65, 0x42, 0x0c, 0x5a, 0x0a, 0x2f, 0x6d, 0x74, 0x6e, 0x5f, 0x70, 0x72, 0x6f, 0x74, 0x6f,
	0x62, 0x06, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x33,
}

var (
	file_bba_proto_rawDescOnce sync.Once
	file_bba_proto_rawDescData = file_bba_proto_rawDesc
)

func file_bba_proto_rawDescGZIP() []byte {
	file_bba_proto_rawDescOnce.Do(func() {
		file_bba_proto_rawDescData = protoimpl.X.CompressGZIP(file_bba_proto_rawDescData)
	})
	return file_bba_proto_rawDescData
}

var file_bba_proto_msgTypes = make([]protoimpl.MessageInfo, 3)
var file_bba_proto_goTypes = []interface{}{
	(*BBAMessage)(nil),  // 0: mtn_proto.BBAMessage
	(*BvalRequest)(nil), // 1: mtn_proto.BvalRequest
	(*AuxRequest)(nil),  // 2: mtn_proto.AuxRequest
}
var file_bba_proto_depIdxs = []int32{
	1, // 0: mtn_proto.BBAMessage.bval_request:type_name -> mtn_proto.BvalRequest
	2, // 1: mtn_proto.BBAMessage.aux_request:type_name -> mtn_proto.AuxRequest
	2, // [2:2] is the sub-list for method output_type
	2, // [2:2] is the sub-list for method input_type
	2, // [2:2] is the sub-list for extension type_name
	2, // [2:2] is the sub-list for extension extendee
	0, // [0:2] is the sub-list for field type_name
}

func init() { file_bba_proto_init() }
func file_bba_proto_init() {
	if File_bba_proto != nil {
		return
	}
	if !protoimpl.UnsafeEnabled {
		file_bba_proto_msgTypes[0].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*BBAMessage); i {
			case 0:
				return &v.state
			case 1:
				return &v.sizeCache
			case 2:
				return &v.unknownFields
			default:
				return nil
			}
		}
		file_bba_proto_msgTypes[1].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*BvalRequest); i {
			case 0:
				return &v.state
			case 1:
				return &v.sizeCache
			case 2:
				return &v.unknownFields
			default:
				return nil
			}
		}
		file_bba_proto_msgTypes[2].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*AuxRequest); i {
			case 0:
				return &v.state
			case 1:
				return &v.sizeCache
			case 2:
				return &v.unknownFields
			default:
				return nil
			}
		}
	}
	file_bba_proto_msgTypes[0].OneofWrappers = []interface{}{
		(*BBAMessage_BvalRequest)(nil),
		(*BBAMessage_AuxRequest)(nil),
	}
	type x struct{}
	out := protoimpl.TypeBuilder{
		File: protoimpl.DescBuilder{
			GoPackagePath: reflect.TypeOf(x{}).PkgPath(),
			RawDescriptor: file_bba_proto_rawDesc,
			NumEnums:      0,
			NumMessages:   3,
			NumExtensions: 0,
			NumServices:   0,
		},
		GoTypes:           file_bba_proto_goTypes,
		DependencyIndexes: file_bba_proto_depIdxs,
		MessageInfos:      file_bba_proto_msgTypes,
	}.Build()
	File_bba_proto = out.File
	file_bba_proto_rawDesc = nil
	file_bba_proto_goTypes = nil
	file_bba_proto_depIdxs = nil
}
