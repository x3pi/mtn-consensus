// Code generated by protoc-gen-go. DO NOT EDIT.
// versions:
// 	protoc-gen-go v1.25.0-devel
// 	protoc        v3.20.3
// source: stats.proto

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

type Stats struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	TotalMemory   uint64          `protobuf:"varint,1,opt,name=TotalMemory,proto3" json:"TotalMemory,omitempty"`
	HeapMemory    uint64          `protobuf:"varint,2,opt,name=HeapMemory,proto3" json:"HeapMemory,omitempty"`
	NumGoroutines int32           `protobuf:"varint,3,opt,name=NumGoroutines,proto3" json:"NumGoroutines,omitempty"`
	Uptime        uint64          `protobuf:"varint,4,opt,name=Uptime,proto3" json:"Uptime,omitempty"`
	Network       *NetworkStats   `protobuf:"bytes,5,opt,name=Network,proto3" json:"Network,omitempty"`
	DB            []*LevelDBStats `protobuf:"bytes,6,rep,name=DB,proto3" json:"DB,omitempty"`
}

func (x *Stats) Reset() {
	*x = Stats{}
	if protoimpl.UnsafeEnabled {
		mi := &file_stats_proto_msgTypes[0]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *Stats) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*Stats) ProtoMessage() {}

func (x *Stats) ProtoReflect() protoreflect.Message {
	mi := &file_stats_proto_msgTypes[0]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use Stats.ProtoReflect.Descriptor instead.
func (*Stats) Descriptor() ([]byte, []int) {
	return file_stats_proto_rawDescGZIP(), []int{0}
}

func (x *Stats) GetTotalMemory() uint64 {
	if x != nil {
		return x.TotalMemory
	}
	return 0
}

func (x *Stats) GetHeapMemory() uint64 {
	if x != nil {
		return x.HeapMemory
	}
	return 0
}

func (x *Stats) GetNumGoroutines() int32 {
	if x != nil {
		return x.NumGoroutines
	}
	return 0
}

func (x *Stats) GetUptime() uint64 {
	if x != nil {
		return x.Uptime
	}
	return 0
}

func (x *Stats) GetNetwork() *NetworkStats {
	if x != nil {
		return x.Network
	}
	return nil
}

func (x *Stats) GetDB() []*LevelDBStats {
	if x != nil {
		return x.DB
	}
	return nil
}

type NetworkStats struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	TotalConnectionByType map[string]int32 `protobuf:"bytes,1,rep,name=TotalConnectionByType,proto3" json:"TotalConnectionByType,omitempty" protobuf_key:"bytes,1,opt,name=key,proto3" protobuf_val:"varint,2,opt,name=value,proto3"`
}

func (x *NetworkStats) Reset() {
	*x = NetworkStats{}
	if protoimpl.UnsafeEnabled {
		mi := &file_stats_proto_msgTypes[1]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *NetworkStats) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*NetworkStats) ProtoMessage() {}

func (x *NetworkStats) ProtoReflect() protoreflect.Message {
	mi := &file_stats_proto_msgTypes[1]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use NetworkStats.ProtoReflect.Descriptor instead.
func (*NetworkStats) Descriptor() ([]byte, []int) {
	return file_stats_proto_rawDescGZIP(), []int{1}
}

func (x *NetworkStats) GetTotalConnectionByType() map[string]int32 {
	if x != nil {
		return x.TotalConnectionByType
	}
	return nil
}

type LevelDBStats struct {
	state         protoimpl.MessageState
	sizeCache     protoimpl.SizeCache
	unknownFields protoimpl.UnknownFields

	LevelSizes        []uint64 `protobuf:"varint,1,rep,packed,name=LevelSizes,proto3" json:"LevelSizes,omitempty"`
	LevelTablesCounts []uint64 `protobuf:"varint,2,rep,packed,name=LevelTablesCounts,proto3" json:"LevelTablesCounts,omitempty"`
	LevelRead         []uint64 `protobuf:"varint,3,rep,packed,name=LevelRead,proto3" json:"LevelRead,omitempty"`
	LevelWrite        []uint64 `protobuf:"varint,4,rep,packed,name=LevelWrite,proto3" json:"LevelWrite,omitempty"`
	LevelDurations    []uint64 `protobuf:"varint,5,rep,packed,name=LevelDurations,proto3" json:"LevelDurations,omitempty"`
	MemComp           uint32   `protobuf:"varint,6,opt,name=MemComp,proto3" json:"MemComp,omitempty"`
	Level0Comp        uint32   `protobuf:"varint,7,opt,name=Level0Comp,proto3" json:"Level0Comp,omitempty"`
	NonLevel0Comp     uint32   `protobuf:"varint,8,opt,name=NonLevel0Comp,proto3" json:"NonLevel0Comp,omitempty"`
	SeekComp          uint32   `protobuf:"varint,9,opt,name=SeekComp,proto3" json:"SeekComp,omitempty"`
	AliveSnapshots    int32    `protobuf:"varint,10,opt,name=AliveSnapshots,proto3" json:"AliveSnapshots,omitempty"`
	AliveIterators    int32    `protobuf:"varint,11,opt,name=AliveIterators,proto3" json:"AliveIterators,omitempty"`
	IOWrite           uint64   `protobuf:"varint,12,opt,name=IOWrite,proto3" json:"IOWrite,omitempty"`
	IORead            uint64   `protobuf:"varint,13,opt,name=IORead,proto3" json:"IORead,omitempty"`
	BlockCacheSize    int32    `protobuf:"varint,14,opt,name=BlockCacheSize,proto3" json:"BlockCacheSize,omitempty"`
	OpenedTablesCount int32    `protobuf:"varint,15,opt,name=OpenedTablesCount,proto3" json:"OpenedTablesCount,omitempty"`
	Path              string   `protobuf:"bytes,16,opt,name=Path,proto3" json:"Path,omitempty"`
}

func (x *LevelDBStats) Reset() {
	*x = LevelDBStats{}
	if protoimpl.UnsafeEnabled {
		mi := &file_stats_proto_msgTypes[2]
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		ms.StoreMessageInfo(mi)
	}
}

func (x *LevelDBStats) String() string {
	return protoimpl.X.MessageStringOf(x)
}

func (*LevelDBStats) ProtoMessage() {}

func (x *LevelDBStats) ProtoReflect() protoreflect.Message {
	mi := &file_stats_proto_msgTypes[2]
	if protoimpl.UnsafeEnabled && x != nil {
		ms := protoimpl.X.MessageStateOf(protoimpl.Pointer(x))
		if ms.LoadMessageInfo() == nil {
			ms.StoreMessageInfo(mi)
		}
		return ms
	}
	return mi.MessageOf(x)
}

// Deprecated: Use LevelDBStats.ProtoReflect.Descriptor instead.
func (*LevelDBStats) Descriptor() ([]byte, []int) {
	return file_stats_proto_rawDescGZIP(), []int{2}
}

func (x *LevelDBStats) GetLevelSizes() []uint64 {
	if x != nil {
		return x.LevelSizes
	}
	return nil
}

func (x *LevelDBStats) GetLevelTablesCounts() []uint64 {
	if x != nil {
		return x.LevelTablesCounts
	}
	return nil
}

func (x *LevelDBStats) GetLevelRead() []uint64 {
	if x != nil {
		return x.LevelRead
	}
	return nil
}

func (x *LevelDBStats) GetLevelWrite() []uint64 {
	if x != nil {
		return x.LevelWrite
	}
	return nil
}

func (x *LevelDBStats) GetLevelDurations() []uint64 {
	if x != nil {
		return x.LevelDurations
	}
	return nil
}

func (x *LevelDBStats) GetMemComp() uint32 {
	if x != nil {
		return x.MemComp
	}
	return 0
}

func (x *LevelDBStats) GetLevel0Comp() uint32 {
	if x != nil {
		return x.Level0Comp
	}
	return 0
}

func (x *LevelDBStats) GetNonLevel0Comp() uint32 {
	if x != nil {
		return x.NonLevel0Comp
	}
	return 0
}

func (x *LevelDBStats) GetSeekComp() uint32 {
	if x != nil {
		return x.SeekComp
	}
	return 0
}

func (x *LevelDBStats) GetAliveSnapshots() int32 {
	if x != nil {
		return x.AliveSnapshots
	}
	return 0
}

func (x *LevelDBStats) GetAliveIterators() int32 {
	if x != nil {
		return x.AliveIterators
	}
	return 0
}

func (x *LevelDBStats) GetIOWrite() uint64 {
	if x != nil {
		return x.IOWrite
	}
	return 0
}

func (x *LevelDBStats) GetIORead() uint64 {
	if x != nil {
		return x.IORead
	}
	return 0
}

func (x *LevelDBStats) GetBlockCacheSize() int32 {
	if x != nil {
		return x.BlockCacheSize
	}
	return 0
}

func (x *LevelDBStats) GetOpenedTablesCount() int32 {
	if x != nil {
		return x.OpenedTablesCount
	}
	return 0
}

func (x *LevelDBStats) GetPath() string {
	if x != nil {
		return x.Path
	}
	return ""
}

var File_stats_proto protoreflect.FileDescriptor

var file_stats_proto_rawDesc = []byte{
	0x0a, 0x0b, 0x73, 0x74, 0x61, 0x74, 0x73, 0x2e, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x12, 0x05, 0x73,
	0x74, 0x61, 0x74, 0x73, 0x22, 0xdb, 0x01, 0x0a, 0x05, 0x53, 0x74, 0x61, 0x74, 0x73, 0x12, 0x20,
	0x0a, 0x0b, 0x54, 0x6f, 0x74, 0x61, 0x6c, 0x4d, 0x65, 0x6d, 0x6f, 0x72, 0x79, 0x18, 0x01, 0x20,
	0x01, 0x28, 0x04, 0x52, 0x0b, 0x54, 0x6f, 0x74, 0x61, 0x6c, 0x4d, 0x65, 0x6d, 0x6f, 0x72, 0x79,
	0x12, 0x1e, 0x0a, 0x0a, 0x48, 0x65, 0x61, 0x70, 0x4d, 0x65, 0x6d, 0x6f, 0x72, 0x79, 0x18, 0x02,
	0x20, 0x01, 0x28, 0x04, 0x52, 0x0a, 0x48, 0x65, 0x61, 0x70, 0x4d, 0x65, 0x6d, 0x6f, 0x72, 0x79,
	0x12, 0x24, 0x0a, 0x0d, 0x4e, 0x75, 0x6d, 0x47, 0x6f, 0x72, 0x6f, 0x75, 0x74, 0x69, 0x6e, 0x65,
	0x73, 0x18, 0x03, 0x20, 0x01, 0x28, 0x05, 0x52, 0x0d, 0x4e, 0x75, 0x6d, 0x47, 0x6f, 0x72, 0x6f,
	0x75, 0x74, 0x69, 0x6e, 0x65, 0x73, 0x12, 0x16, 0x0a, 0x06, 0x55, 0x70, 0x74, 0x69, 0x6d, 0x65,
	0x18, 0x04, 0x20, 0x01, 0x28, 0x04, 0x52, 0x06, 0x55, 0x70, 0x74, 0x69, 0x6d, 0x65, 0x12, 0x2d,
	0x0a, 0x07, 0x4e, 0x65, 0x74, 0x77, 0x6f, 0x72, 0x6b, 0x18, 0x05, 0x20, 0x01, 0x28, 0x0b, 0x32,
	0x13, 0x2e, 0x73, 0x74, 0x61, 0x74, 0x73, 0x2e, 0x4e, 0x65, 0x74, 0x77, 0x6f, 0x72, 0x6b, 0x53,
	0x74, 0x61, 0x74, 0x73, 0x52, 0x07, 0x4e, 0x65, 0x74, 0x77, 0x6f, 0x72, 0x6b, 0x12, 0x23, 0x0a,
	0x02, 0x44, 0x42, 0x18, 0x06, 0x20, 0x03, 0x28, 0x0b, 0x32, 0x13, 0x2e, 0x73, 0x74, 0x61, 0x74,
	0x73, 0x2e, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x44, 0x42, 0x53, 0x74, 0x61, 0x74, 0x73, 0x52, 0x02,
	0x44, 0x42, 0x22, 0xbe, 0x01, 0x0a, 0x0c, 0x4e, 0x65, 0x74, 0x77, 0x6f, 0x72, 0x6b, 0x53, 0x74,
	0x61, 0x74, 0x73, 0x12, 0x64, 0x0a, 0x15, 0x54, 0x6f, 0x74, 0x61, 0x6c, 0x43, 0x6f, 0x6e, 0x6e,
	0x65, 0x63, 0x74, 0x69, 0x6f, 0x6e, 0x42, 0x79, 0x54, 0x79, 0x70, 0x65, 0x18, 0x01, 0x20, 0x03,
	0x28, 0x0b, 0x32, 0x2e, 0x2e, 0x73, 0x74, 0x61, 0x74, 0x73, 0x2e, 0x4e, 0x65, 0x74, 0x77, 0x6f,
	0x72, 0x6b, 0x53, 0x74, 0x61, 0x74, 0x73, 0x2e, 0x54, 0x6f, 0x74, 0x61, 0x6c, 0x43, 0x6f, 0x6e,
	0x6e, 0x65, 0x63, 0x74, 0x69, 0x6f, 0x6e, 0x42, 0x79, 0x54, 0x79, 0x70, 0x65, 0x45, 0x6e, 0x74,
	0x72, 0x79, 0x52, 0x15, 0x54, 0x6f, 0x74, 0x61, 0x6c, 0x43, 0x6f, 0x6e, 0x6e, 0x65, 0x63, 0x74,
	0x69, 0x6f, 0x6e, 0x42, 0x79, 0x54, 0x79, 0x70, 0x65, 0x1a, 0x48, 0x0a, 0x1a, 0x54, 0x6f, 0x74,
	0x61, 0x6c, 0x43, 0x6f, 0x6e, 0x6e, 0x65, 0x63, 0x74, 0x69, 0x6f, 0x6e, 0x42, 0x79, 0x54, 0x79,
	0x70, 0x65, 0x45, 0x6e, 0x74, 0x72, 0x79, 0x12, 0x10, 0x0a, 0x03, 0x6b, 0x65, 0x79, 0x18, 0x01,
	0x20, 0x01, 0x28, 0x09, 0x52, 0x03, 0x6b, 0x65, 0x79, 0x12, 0x14, 0x0a, 0x05, 0x76, 0x61, 0x6c,
	0x75, 0x65, 0x18, 0x02, 0x20, 0x01, 0x28, 0x05, 0x52, 0x05, 0x76, 0x61, 0x6c, 0x75, 0x65, 0x3a,
	0x02, 0x38, 0x01, 0x22, 0xaa, 0x04, 0x0a, 0x0c, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x44, 0x42, 0x53,
	0x74, 0x61, 0x74, 0x73, 0x12, 0x1e, 0x0a, 0x0a, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x53, 0x69, 0x7a,
	0x65, 0x73, 0x18, 0x01, 0x20, 0x03, 0x28, 0x04, 0x52, 0x0a, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x53,
	0x69, 0x7a, 0x65, 0x73, 0x12, 0x2c, 0x0a, 0x11, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x54, 0x61, 0x62,
	0x6c, 0x65, 0x73, 0x43, 0x6f, 0x75, 0x6e, 0x74, 0x73, 0x18, 0x02, 0x20, 0x03, 0x28, 0x04, 0x52,
	0x11, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x73, 0x43, 0x6f, 0x75, 0x6e,
	0x74, 0x73, 0x12, 0x1c, 0x0a, 0x09, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x52, 0x65, 0x61, 0x64, 0x18,
	0x03, 0x20, 0x03, 0x28, 0x04, 0x52, 0x09, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x52, 0x65, 0x61, 0x64,
	0x12, 0x1e, 0x0a, 0x0a, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x57, 0x72, 0x69, 0x74, 0x65, 0x18, 0x04,
	0x20, 0x03, 0x28, 0x04, 0x52, 0x0a, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x57, 0x72, 0x69, 0x74, 0x65,
	0x12, 0x26, 0x0a, 0x0e, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x44, 0x75, 0x72, 0x61, 0x74, 0x69, 0x6f,
	0x6e, 0x73, 0x18, 0x05, 0x20, 0x03, 0x28, 0x04, 0x52, 0x0e, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x44,
	0x75, 0x72, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x73, 0x12, 0x18, 0x0a, 0x07, 0x4d, 0x65, 0x6d, 0x43,
	0x6f, 0x6d, 0x70, 0x18, 0x06, 0x20, 0x01, 0x28, 0x0d, 0x52, 0x07, 0x4d, 0x65, 0x6d, 0x43, 0x6f,
	0x6d, 0x70, 0x12, 0x1e, 0x0a, 0x0a, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x30, 0x43, 0x6f, 0x6d, 0x70,
	0x18, 0x07, 0x20, 0x01, 0x28, 0x0d, 0x52, 0x0a, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x30, 0x43, 0x6f,
	0x6d, 0x70, 0x12, 0x24, 0x0a, 0x0d, 0x4e, 0x6f, 0x6e, 0x4c, 0x65, 0x76, 0x65, 0x6c, 0x30, 0x43,
	0x6f, 0x6d, 0x70, 0x18, 0x08, 0x20, 0x01, 0x28, 0x0d, 0x52, 0x0d, 0x4e, 0x6f, 0x6e, 0x4c, 0x65,
	0x76, 0x65, 0x6c, 0x30, 0x43, 0x6f, 0x6d, 0x70, 0x12, 0x1a, 0x0a, 0x08, 0x53, 0x65, 0x65, 0x6b,
	0x43, 0x6f, 0x6d, 0x70, 0x18, 0x09, 0x20, 0x01, 0x28, 0x0d, 0x52, 0x08, 0x53, 0x65, 0x65, 0x6b,
	0x43, 0x6f, 0x6d, 0x70, 0x12, 0x26, 0x0a, 0x0e, 0x41, 0x6c, 0x69, 0x76, 0x65, 0x53, 0x6e, 0x61,
	0x70, 0x73, 0x68, 0x6f, 0x74, 0x73, 0x18, 0x0a, 0x20, 0x01, 0x28, 0x05, 0x52, 0x0e, 0x41, 0x6c,
	0x69, 0x76, 0x65, 0x53, 0x6e, 0x61, 0x70, 0x73, 0x68, 0x6f, 0x74, 0x73, 0x12, 0x26, 0x0a, 0x0e,
	0x41, 0x6c, 0x69, 0x76, 0x65, 0x49, 0x74, 0x65, 0x72, 0x61, 0x74, 0x6f, 0x72, 0x73, 0x18, 0x0b,
	0x20, 0x01, 0x28, 0x05, 0x52, 0x0e, 0x41, 0x6c, 0x69, 0x76, 0x65, 0x49, 0x74, 0x65, 0x72, 0x61,
	0x74, 0x6f, 0x72, 0x73, 0x12, 0x18, 0x0a, 0x07, 0x49, 0x4f, 0x57, 0x72, 0x69, 0x74, 0x65, 0x18,
	0x0c, 0x20, 0x01, 0x28, 0x04, 0x52, 0x07, 0x49, 0x4f, 0x57, 0x72, 0x69, 0x74, 0x65, 0x12, 0x16,
	0x0a, 0x06, 0x49, 0x4f, 0x52, 0x65, 0x61, 0x64, 0x18, 0x0d, 0x20, 0x01, 0x28, 0x04, 0x52, 0x06,
	0x49, 0x4f, 0x52, 0x65, 0x61, 0x64, 0x12, 0x26, 0x0a, 0x0e, 0x42, 0x6c, 0x6f, 0x63, 0x6b, 0x43,
	0x61, 0x63, 0x68, 0x65, 0x53, 0x69, 0x7a, 0x65, 0x18, 0x0e, 0x20, 0x01, 0x28, 0x05, 0x52, 0x0e,
	0x42, 0x6c, 0x6f, 0x63, 0x6b, 0x43, 0x61, 0x63, 0x68, 0x65, 0x53, 0x69, 0x7a, 0x65, 0x12, 0x2c,
	0x0a, 0x11, 0x4f, 0x70, 0x65, 0x6e, 0x65, 0x64, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x73, 0x43, 0x6f,
	0x75, 0x6e, 0x74, 0x18, 0x0f, 0x20, 0x01, 0x28, 0x05, 0x52, 0x11, 0x4f, 0x70, 0x65, 0x6e, 0x65,
	0x64, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x73, 0x43, 0x6f, 0x75, 0x6e, 0x74, 0x12, 0x12, 0x0a, 0x04,
	0x50, 0x61, 0x74, 0x68, 0x18, 0x10, 0x20, 0x01, 0x28, 0x09, 0x52, 0x04, 0x50, 0x61, 0x74, 0x68,
	0x42, 0x31, 0x0a, 0x23, 0x63, 0x6f, 0x6d, 0x2e, 0x6d, 0x65, 0x74, 0x61, 0x5f, 0x6e, 0x6f, 0x64,
	0x65, 0x2e, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x73, 0x2e, 0x63, 0x6f, 0x6d, 0x70, 0x69, 0x6c, 0x65,
	0x64, 0x2e, 0x73, 0x74, 0x61, 0x74, 0x73, 0x5a, 0x0a, 0x2f, 0x6d, 0x74, 0x6e, 0x5f, 0x70, 0x72,
	0x6f, 0x74, 0x6f, 0x62, 0x06, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x33,
}

var (
	file_stats_proto_rawDescOnce sync.Once
	file_stats_proto_rawDescData = file_stats_proto_rawDesc
)

func file_stats_proto_rawDescGZIP() []byte {
	file_stats_proto_rawDescOnce.Do(func() {
		file_stats_proto_rawDescData = protoimpl.X.CompressGZIP(file_stats_proto_rawDescData)
	})
	return file_stats_proto_rawDescData
}

var file_stats_proto_msgTypes = make([]protoimpl.MessageInfo, 4)
var file_stats_proto_goTypes = []interface{}{
	(*Stats)(nil),        // 0: stats.Stats
	(*NetworkStats)(nil), // 1: stats.NetworkStats
	(*LevelDBStats)(nil), // 2: stats.LevelDBStats
	nil,                  // 3: stats.NetworkStats.TotalConnectionByTypeEntry
}
var file_stats_proto_depIdxs = []int32{
	1, // 0: stats.Stats.Network:type_name -> stats.NetworkStats
	2, // 1: stats.Stats.DB:type_name -> stats.LevelDBStats
	3, // 2: stats.NetworkStats.TotalConnectionByType:type_name -> stats.NetworkStats.TotalConnectionByTypeEntry
	3, // [3:3] is the sub-list for method output_type
	3, // [3:3] is the sub-list for method input_type
	3, // [3:3] is the sub-list for extension type_name
	3, // [3:3] is the sub-list for extension extendee
	0, // [0:3] is the sub-list for field type_name
}

func init() { file_stats_proto_init() }
func file_stats_proto_init() {
	if File_stats_proto != nil {
		return
	}
	if !protoimpl.UnsafeEnabled {
		file_stats_proto_msgTypes[0].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*Stats); i {
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
		file_stats_proto_msgTypes[1].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*NetworkStats); i {
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
		file_stats_proto_msgTypes[2].Exporter = func(v interface{}, i int) interface{} {
			switch v := v.(*LevelDBStats); i {
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
			RawDescriptor: file_stats_proto_rawDesc,
			NumEnums:      0,
			NumMessages:   4,
			NumExtensions: 0,
			NumServices:   0,
		},
		GoTypes:           file_stats_proto_goTypes,
		DependencyIndexes: file_stats_proto_depIdxs,
		MessageInfos:      file_stats_proto_msgTypes,
	}.Build()
	File_stats_proto = out.File
	file_stats_proto_rawDesc = nil
	file_stats_proto_goTypes = nil
	file_stats_proto_depIdxs = nil
}
