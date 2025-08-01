// Code generated by protoc-gen-go. DO NOT EDIT.
// versions:
// 	protoc-gen-go v1.25.0-devel
// 	protoc        v3.20.3
// source: event_log.proto

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

type EventLog struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	TransactionHash []byte   `protobuf:"bytes,2,opt,name=TransactionHash,proto3" json:"TransactionHash,omitempty"`
	Address         []byte   `protobuf:"bytes,3,opt,name=Address,proto3" json:"Address,omitempty"`
	Data            []byte   `protobuf:"bytes,4,opt,name=Data,proto3" json:"Data,omitempty"`
	Topics          [][]byte `protobuf:"bytes,5,rep,name=Topics,proto3" json:"Topics,omitempty"`
}

func (x *EventLog) Reset() {
	*x = EventLog{}
	if protoimpl.UnsafeEnabled {
		mi := &file_event_log_proto_msgTypes[0]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *EventLog) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*EventLog) ProtoMessage() {}

func (x *EventLog) ProtoReflect() protoreflect.Message {
	mi := &file_event_log_proto_msgTypes[0]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use EventLog.ProtoReflect.Descriptor instead.
func (*EventLog) Descriptor() ([]byte, []int) {
	return file_event_log_proto_rawDescGZIP(), []int{0}
}

func (x *EventLog) GetTransactionHash() []byte {
	if x != nil {
		return x.TransactionHash
	}
	return nil
}

func (x *EventLog) GetAddress() []byte {
	if x != nil {
		return x.Address
	}
	return nil
}

func (x *EventLog) GetData() []byte {
	if x != nil {
		return x.Data
	}
	return nil
}

func (x *EventLog) GetTopics() [][]byte {
	if x != nil {
		return x.Topics
	}
	return nil
}

type EventLogsHashData struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	Hashes [][]byte `protobuf:"bytes,1,rep,name=Hashes,proto3" json:"Hashes,omitempty"`
}

func (x *EventLogsHashData) Reset() {
	*x = EventLogsHashData{}
	if protoimpl.UnsafeEnabled {
		mi := &file_event_log_proto_msgTypes[1]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *EventLogsHashData) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*EventLogsHashData) ProtoMessage() {}

func (x *EventLogsHashData) ProtoReflect() protoreflect.Message {
	mi := &file_event_log_proto_msgTypes[1]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use EventLogsHashData.ProtoReflect.Descriptor instead.
func (*EventLogsHashData) Descriptor() ([]byte, []int) {
	return file_event_log_proto_rawDescGZIP(), []int{1}
}

func (x *EventLogsHashData) GetHashes() [][]byte {
	if x != nil {
		return x.Hashes
	}
	return nil
}

type EventLogs struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	EventLogs []*EventLog `protobuf:"bytes,1,rep,name=EventLogs,proto3" json:"EventLogs,omitempty"`
}

func (x *EventLogs) Reset() {
	*x = EventLogs{}
	if protoimpl.UnsafeEnabled {
		mi := &file_event_log_proto_msgTypes[2]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *EventLogs) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*EventLogs) ProtoMessage() {}

func (x *EventLogs) ProtoReflect() protoreflect.Message {
	mi := &file_event_log_proto_msgTypes[2]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use EventLogs.ProtoReflect.Descriptor instead.
func (*EventLogs) Descriptor() ([]byte, []int) {
	return file_event_log_proto_rawDescGZIP(), []int{2}
}

func (x *EventLogs) GetEventLogs() []*EventLog {
	if x != nil {
		return x.EventLogs
	}
	return nil
}

var File_event_log_proto protoreflect.FileDescriptor

var file_event_log_proto_rawDesc = []byte{
	0x0a, 0x0f, 0x65, 0x76, 0x65, 0x6e, 0x74, 0x5f, 0x6c, 0x6f, 0x67, 0x2e, 0x70, 0x72, 0x6f, 0x74,
	0x6f, 0x12, 0x09, 0x65, 0x76, 0x65, 0x6e, 0x74, 0x5f, 0x6c, 0x6f, 0x67, 0x22, 0x7a, 0x0a, 0x08,
	0x45, 0x76, 0x65, 0x6e, 0x74, 0x4c, 0x6f, 0x67, 0x12, 0x28, 0x0a, 0x0f, 0x54, 0x72, 0x61, 0x6e,
	0x73, 0x61, 0x63, 0x74, 0x69, 0x6f, 0x6e, 0x48, 0x61, 0x73, 0x68, 0x18, 0x02, 0x20, 0x01, 0x28,
	0x0c, 0x52, 0x0f, 0x54, 0x72, 0x61, 0x6e, 0x73, 0x61, 0x63, 0x74, 0x69, 0x6f, 0x6e, 0x48, 0x61,
	0x73, 0x68, 0x12, 0x18, 0x0a, 0x07, 0x41, 0x64, 0x64, 0x72, 0x65, 0x73, 0x73, 0x18, 0x03, 0x20,
	0x01, 0x28, 0x0c, 0x52, 0x07, 0x41, 0x64, 0x64, 0x72, 0x65, 0x73, 0x73, 0x12, 0x12, 0x0a, 0x04,
	0x44, 0x61, 0x74, 0x61, 0x18, 0x04, 0x20, 0x01, 0x28, 0x0c, 0x52, 0x04, 0x44, 0x61, 0x74, 0x61,
	0x12, 0x16, 0x0a, 0x06, 0x54, 0x6f, 0x70, 0x69, 0x63, 0x73, 0x18, 0x05, 0x20, 0x03, 0x28, 0x0c,
	0x52, 0x06, 0x54, 0x6f, 0x70, 0x69, 0x63, 0x73, 0x22, 0x2b, 0x0a, 0x11, 0x45, 0x76, 0x65, 0x6e,
	0x74, 0x4c, 0x6f, 0x67, 0x73, 0x48, 0x61, 0x73, 0x68, 0x44, 0x61, 0x74, 0x61, 0x12, 0x16, 0x0a,
	0x06, 0x48, 0x61, 0x73, 0x68, 0x65, 0x73, 0x18, 0x01, 0x20, 0x03, 0x28, 0x0c, 0x52, 0x06, 0x48,
	0x61, 0x73, 0x68, 0x65, 0x73, 0x22, 0x3e, 0x0a, 0x09, 0x45, 0x76, 0x65, 0x6e, 0x74, 0x4c, 0x6f,
	0x67, 0x73, 0x12, 0x31, 0x0a, 0x09, 0x45, 0x76, 0x65, 0x6e, 0x74, 0x4c, 0x6f, 0x67, 0x73, 0x18,
	0x01, 0x20, 0x03, 0x28, 0x0b, 0x32, 0x13, 0x2e, 0x65, 0x76, 0x65, 0x6e, 0x74, 0x5f, 0x6c, 0x6f,
	0x67, 0x2e, 0x45, 0x76, 0x65, 0x6e, 0x74, 0x4c, 0x6f, 0x67, 0x52, 0x09, 0x45, 0x76, 0x65, 0x6e,
	0x74, 0x4c, 0x6f, 0x67, 0x73, 0x42, 0x35, 0x0a, 0x27, 0x63, 0x6f, 0x6d, 0x2e, 0x6d, 0x65, 0x74,
	0x61, 0x5f, 0x6e, 0x6f, 0x64, 0x65, 0x2e, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x73, 0x2e, 0x63, 0x6f,
	0x6d, 0x70, 0x69, 0x6c, 0x65, 0x64, 0x2e, 0x65, 0x76, 0x65, 0x6e, 0x74, 0x5f, 0x6c, 0x6f, 0x67,
	0x5a, 0x0a, 0x2f, 0x6d, 0x74, 0x6e, 0x5f, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x62, 0x06, 0x70, 0x72,
	0x6f, 0x74, 0x6f, 0x33,
}

var (
	file_event_log_proto_rawDescOnce sync.Once
	file_event_log_proto_rawDescData = file_event_log_proto_rawDesc
)

func file_event_log_proto_rawDescGZIP() []byte {
	file_event_log_proto_rawDescOnce.Do(func() {
		file_event_log_proto_rawDescData = protoimpl.X.CompressGZIP(file_event_log_proto_rawDescData)
	})
	return file_event_log_proto_rawDescData
}

var file_event_log_proto_msgTypes = make([]protoimpl.MessageInfo, 3)
var file_event_log_proto_goTypes = []interface{}{
	(*EventLog)(nil),          // 0: event_log.EventLog
	(*EventLogsHashData)(nil), // 1: event_log.EventLogsHashData
	(*EventLogs)(nil),         // 2: event_log.EventLogs
}
var file_event_log_proto_depIdxs = []int32{
	0, // 0: event_log.EventLogs.EventLogs:type_name -> event_log.EventLog
	1, // [1:1] is the sub-list for method output_type
	1, // [1:1] is the sub-list for method input_type
	1, // [1:1] is the sub-list for extension type_name
	1, // [1:1] is the sub-list for extension extendee
	0, // [0:1] is the sub-list for field type_name
}

func init() { file_event_log_proto_init() }
func file_event_log_proto_init() {
	if File_event_log_proto != nil {
		return
	}
	if !protoimpl.UnsafeEnabled {
		file_event_log_proto_msgTypes[0].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*EventLog); i {
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
		file_event_log_proto_msgTypes[1].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*EventLogsHashData); i {
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
		file_event_log_proto_msgTypes[2].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*EventLogs); i {
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
	type x struct{}
	out := protoimpl.TypeBuilder{
		File: protoimpl.DescBuilder{
			GoPackagePath: reflect.TypeOf(x{}).PkgPath(),
			RawDescriptor: file_event_log_proto_rawDesc,
			NumEnums:      0,
			NumMessages:   3,
			NumExtensions: 0,
			NumServices:   0,
		},
		GoTypes:           file_event_log_proto_goTypes,
		DependencyIndexes: file_event_log_proto_depIdxs,
		MessageInfos:      file_event_log_proto_msgTypes,
	}.Build()
	File_event_log_proto = out.File
	file_event_log_proto_rawDesc = nil
	file_event_log_proto_goTypes = nil
	file_event_log_proto_depIdxs = nil
}
