package storage

import (
	// Keep for potential other uses, though NewReader might be removed for borsh.Deserialize
	"context"
	"errors"
	"fmt"
	"io"
	"time"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"github.com/near/borsh-go" // Using borsh for serialization

	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/network"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/libp2p/go-libp2p/core/protocol"
)

// Protocol IDs remain the same
const (
	ProtocolGet         protocol.ID = "/storage/get/1.0.0"
	ProtocolPut         protocol.ID = "/storage/put/1.0.0"
	ProtocolDelete      protocol.ID = "/storage/delete/1.0.0"
	ProtocolBatchPut    protocol.ID = "/storage/batchput/1.0.0"
	ProtocolBatchDelete protocol.ID = "/storage/batchdelete/1.0.0"
	ProtocolGetBackup   protocol.ID = "/storage/getbackup/1.0.0"
)

// Libp2pStorage and Request/Response structs remain the same
type Libp2pStorage struct {
	host         host.Host
	targetPeerID peer.ID
}

type GetRequest struct {
	Key []byte
}
type GetResponse struct {
	Value []byte
	Error string
}

type PutRequest struct {
	Key   []byte
	Value []byte
}
type PutResponse struct {
	Error string
}

type DeleteRequest struct {
	Key []byte
}
type DeleteResponse struct {
	Error string
}

type BatchPutRequest struct {
	KVS [][2][]byte
}
type BatchPutResponse struct {
	Error string
}

type BatchDeleteRequest struct {
	Keys [][]byte
}
type BatchDeleteResponse struct {
	Error string
}

type GetBackupPathRequest struct{}
type GetBackupPathResponse struct {
	Path  string
	Error string
}

func NewLibp2pStorage(localHost host.Host, remotePeerID peer.ID) (*Libp2pStorage, error) {
	if localHost == nil {
		return nil, errors.New("libp2p host không được nil")
	}
	if remotePeerID == "" {
		return nil, errors.New("remote peer ID không được rỗng")
	}
	return &Libp2pStorage{
		host:         localHost,
		targetPeerID: remotePeerID,
	}, nil
}

// Updated sendRequest function
func (ls *Libp2pStorage) sendRequest(protocolID protocol.ID, requestData interface{}, responseData interface{}) error {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	stream, err := ls.host.NewStream(ctx, ls.targetPeerID, protocolID)
	if err != nil {
		return fmt.Errorf("không thể mở stream tới peer %s cho protocol %s: %w", ls.targetPeerID, protocolID, err)
	}
	defer stream.Close()

	serializedReqBytes, err := borsh.Serialize(requestData)
	if err != nil {
		_ = stream.Reset()
		return fmt.Errorf("không thể serialize request bằng borsh cho protocol %s: %w", protocolID, err)
	}

	_, err = stream.Write(serializedReqBytes)
	if err != nil {
		_ = stream.Reset()
		return fmt.Errorf("không thể ghi serialized request vào stream cho protocol %s: %w", protocolID, err)
	}

	err = stream.CloseWrite()
	if err != nil {
		logger.Info("Libp2pStorage: error closing write stream for protocol %s: %v\n", protocolID, err)
	}

	responseBytes, err := io.ReadAll(stream)
	if err != nil {
		if err == io.EOF && len(responseBytes) == 0 {
			_ = stream.Reset()
			return fmt.Errorf("lỗi đọc response từ stream cho protocol %s: EOF không có dữ liệu", protocolID)
		} else if err != io.EOF {
			_ = stream.Reset()
			return fmt.Errorf("lỗi đọc response từ stream cho protocol %s: %w", protocolID, err)
		}
	}

	if len(responseBytes) == 0 && err == io.EOF { // Check if err was EOF and no bytes were read
		// This implies the peer closed the stream without sending any response data.
		// It could be a valid case for some protocols (e.g., fire-and-forget with stream closure as ack),
		// but generally, for request-response, we expect some form of response.
		_ = stream.Reset()
		return fmt.Errorf("không có dữ liệu response từ peer cho protocol %s (stream closed)", protocolID)
	}

	// Corrected: Pass responseBytes directly to borsh.Deserialize
	// if this is what the compiler expects based on the error.
	if err := borsh.Deserialize(responseData, responseBytes); err != nil {
		_ = stream.Reset()
		return fmt.Errorf("không thể deserialize borsh response cho protocol %s từ bytes: %w", protocolID, err)
	}

	return nil
}

// Client-side methods (Get, Put, etc.) remain structurally the same.
func (ls *Libp2pStorage) Get(key []byte) ([]byte, error) {
	req := GetRequest{Key: key}
	var resp GetResponse
	err := ls.sendRequest(ProtocolGet, req, &resp)
	if err != nil {
		return nil, fmt.Errorf("Get thất bại: %w", err)
	}
	if resp.Error != "" {
		return nil, errors.New(resp.Error)
	}
	return resp.Value, nil
}

func (ls *Libp2pStorage) Put(key []byte, value []byte) error {
	req := PutRequest{Key: key, Value: value}
	var resp PutResponse
	err := ls.sendRequest(ProtocolPut, req, &resp)
	if err != nil {
		return fmt.Errorf("Put thất bại: %w", err)
	}
	if resp.Error != "" {
		return errors.New(resp.Error)
	}
	return nil
}

func (ls *Libp2pStorage) Delete(key []byte) error {
	req := DeleteRequest{Key: key}
	var resp DeleteResponse
	err := ls.sendRequest(ProtocolDelete, req, &resp)
	if err != nil {
		return fmt.Errorf("Delete thất bại: %w", err)
	}
	if resp.Error != "" {
		return errors.New(resp.Error)
	}
	return nil
}

func (ls *Libp2pStorage) BatchPut(kvs [][2][]byte) error {
	req := BatchPutRequest{KVS: kvs}
	var resp BatchPutResponse
	err := ls.sendRequest(ProtocolBatchPut, req, &resp)
	if err != nil {
		return fmt.Errorf("BatchPut thất bại: %w", err)
	}
	if resp.Error != "" {
		return errors.New(resp.Error)
	}
	return nil
}

func (ls *Libp2pStorage) BatchDelete(keys [][]byte) error {
	req := BatchDeleteRequest{Keys: keys}
	var resp BatchDeleteResponse
	err := ls.sendRequest(ProtocolBatchDelete, req, &resp)
	if err != nil {
		return fmt.Errorf("BatchDelete thất bại: %w", err)
	}
	if resp.Error != "" {
		return errors.New(resp.Error)
	}
	return nil
}

func (ls *Libp2pStorage) Open() error {
	fmt.Println("Libp2pStorage Open: Kết nối tới remote theo yêu cầu cho mỗi hoạt động.")
	return nil
}

func (ls *Libp2pStorage) Close() error {
	fmt.Println("Libp2pStorage Close: Không có tài nguyên cụ thể nào để giải phóng cho instance này.")
	return nil
}

func (ls *Libp2pStorage) GetBackupPath() string {
	req := GetBackupPathRequest{}
	var resp GetBackupPathResponse
	err := ls.sendRequest(ProtocolGetBackup, req, &resp)
	if err != nil {
		logger.Info("GetBackupPath thất bại khi lấy đường dẫn từ xa: %v\n", err)
		return ""
	}
	if resp.Error != "" {
		logger.Info("Lỗi GetBackupPath từ xa: %s\n", resp.Error)
		return ""
	}
	return resp.Path
}

// --- Updated Server-Side Handlers ---

type RemoteStorageService struct {
	actualStorage Storage
}

func NewRemoteStorageService(storageImpl Storage) *RemoteStorageService {
	return &RemoteStorageService{actualStorage: storageImpl}
}

func (rss *RemoteStorageService) RegisterHandlers(h host.Host) {
	h.SetStreamHandler(ProtocolGet, rss.handleGet)
	h.SetStreamHandler(ProtocolPut, rss.handlePut)
	h.SetStreamHandler(ProtocolDelete, rss.handleDelete)
	h.SetStreamHandler(ProtocolBatchPut, rss.handleBatchPut)
	h.SetStreamHandler(ProtocolBatchDelete, rss.handleBatchDelete)
	h.SetStreamHandler(ProtocolGetBackup, rss.handleGetBackupPath)
}

// Updated generic handler logic
func handleStream[ReqT any, ResT any](s network.Stream, process func(*ReqT) (ResT, error)) {
	defer s.Close()
	var req ReqT
	var resp ResT

	requestBytes, err := io.ReadAll(s)
	if err != nil {
		if !(err == io.EOF && len(requestBytes) > 0) {
			logger.Info("Remote: Lỗi đọc request từ stream: %v\n", err)
			_ = s.Reset()
			return
		}
	}

	if len(requestBytes) == 0 {
		logger.Info("Remote: Không nhận được dữ liệu request.\n")
		// It's possible the client called CloseWrite() immediately without sending data.
		// Depending on protocol, this might be an error or an expected signal.
		_ = s.Reset() // Reset if this is considered an error.
		return
	}

	// Corrected: Pass requestBytes directly to borsh.Deserialize
	// if this is what the compiler expects based on the error.
	if err := borsh.Deserialize(&req, requestBytes); err != nil {
		logger.Info("Remote: Không thể deserialize request bằng borsh từ bytes: %v\n", err)
		_ = s.Reset()
		return
	}

	var appErr error
	resp, appErr = process(&req)

	if appErr != nil {
		switch r := any(&resp).(type) {
		case *GetResponse:
			r.Error = appErr.Error()
		case *PutResponse:
			r.Error = appErr.Error()
		case *DeleteResponse:
			r.Error = appErr.Error()
		case *BatchPutResponse:
			r.Error = appErr.Error()
		case *BatchDeleteResponse:
			r.Error = appErr.Error()
		case *GetBackupPathResponse:
			r.Error = appErr.Error()
		default:
			logger.Info("Remote: Lỗi ứng dụng xử lý request, nhưng kiểu response không có trường Error hoặc không được xử lý: %v\n", appErr)
		}
	}

	serializedRespBytes, err := borsh.Serialize(resp)
	if err != nil {
		logger.Info("Remote: Không thể serialize response bằng borsh: %v\n", err)
		_ = s.Reset()
		return
	}

	_, err = s.Write(serializedRespBytes)
	if err != nil {
		logger.Info("Remote: Không thể ghi serialized response vào stream: %v\n", err)
		_ = s.Reset()
	}
}

// Specific handlers use the generic handleStream
func (rss *RemoteStorageService) handleGet(s network.Stream) {
	handleStream(s, func(req *GetRequest) (GetResponse, error) {
		var resp GetResponse
		value, err := rss.actualStorage.Get(req.Key)
		if err != nil {
			return resp, err
		}
		resp.Value = value
		return resp, nil
	})
}

func (rss *RemoteStorageService) handlePut(s network.Stream) {
	handleStream(s, func(req *PutRequest) (PutResponse, error) {
		err := rss.actualStorage.Put(req.Key, req.Value)
		return PutResponse{}, err
	})
}

func (rss *RemoteStorageService) handleDelete(s network.Stream) {
	handleStream(s, func(req *DeleteRequest) (DeleteResponse, error) {
		err := rss.actualStorage.Delete(req.Key)
		return DeleteResponse{}, err
	})
}

func (rss *RemoteStorageService) handleBatchPut(s network.Stream) {
	handleStream(s, func(req *BatchPutRequest) (BatchPutResponse, error) {
		err := rss.actualStorage.BatchPut(req.KVS)
		return BatchPutResponse{}, err
	})
}

func (rss *RemoteStorageService) handleBatchDelete(s network.Stream) {
	handleStream(s, func(req *BatchDeleteRequest) (BatchDeleteResponse, error) {
		err := rss.actualStorage.BatchDelete(req.Keys)
		return BatchDeleteResponse{}, err
	})
}

func (rss *RemoteStorageService) handleGetBackupPath(s network.Stream) {
	handleStream(s, func(req *GetBackupPathRequest) (GetBackupPathResponse, error) {
		var resp GetBackupPathResponse
		resp.Path = rss.actualStorage.GetBackupPath()
		return resp, nil
	})
}
