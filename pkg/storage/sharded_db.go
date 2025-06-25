package storage

import (
	"bytes"
	"crypto/md5"
	"encoding/binary"
	"fmt"
	"sync"

	"github.com/ethereum/go-ethereum/ethdb"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	"golang.org/x/sync/errgroup"
)

type DBType int

const (
	TypeRocksDB DBType = iota
	TypeLevelDB
)

// RocksDBInterface là interface cho các hoạt động trên RocksDB
type ShardDBInterface interface {
	Open(parallelism int) error
	Get(key []byte) ([]byte, error)
	Put(key, value []byte) error
	Delete(key []byte) error
	BatchPut(keys [][2][]byte) error
	Close() error
	GetAllKeys() ([]string, error)

	// Thêm các phương thức khác nếu cần thiết
}

// Cấu trúc quản lý nhiều instance LevelDB với sharding
type ShardelDB struct {
	shards      []ShardDBInterface
	numShards   int
	parallelism int
	backupPath  string
	dbPath      string
}

// Hàm bổ sung để lấy index của shard
func (s *ShardelDB) getShardIndex(key []byte) uint32 {
	hash := md5.Sum(key)
	index := binary.BigEndian.Uint32(hash[:4]) % uint32(s.numShards)
	return index
}
func (s *ShardelDB) GetBackupPath() string {
	return s.backupPath
}

func (s *ShardelDB) GetDbPath() string {
	return s.dbPath
}

// Tạo mới `ShardelDB`
func NewShardelDB(baseDir string, numShards int, parallelism int, dbType DBType, backupPath string) (*ShardelDB, error) {
	shards := make([]ShardDBInterface, numShards)
	for i := 0; i < numShards; i++ {
		primaryPath := fmt.Sprintf("%s/db_shard_%d", baseDir, i)
		// Chọn loại DB dựa trên dbType
		var shard ShardDBInterface
		switch dbType {
		case TypeRocksDB:
			shard = NewReplicatedLevelDB(primaryPath)
		case TypeLevelDB:
			// Implement NewReplicatedLevelDB
			shard = NewReplicatedLevelDB(primaryPath)
		default:
			return nil, fmt.Errorf("unsupported db type: %d", dbType)
		}
		shards[i] = shard
	}

	return &ShardelDB{
		shards:      shards,
		numShards:   numShards,
		parallelism: parallelism,
		backupPath:  backupPath,
		dbPath:      baseDir,
	}, nil
}

// Mở toàn bộ LevelDB shards
func (s *ShardelDB) Open() error {
	for _, shard := range s.shards {
		if err := shard.Open(s.parallelism); err != nil {
			return err
		}
	}
	return nil
}

// Băm key để quyết định lưu vào shard nào
func (s *ShardelDB) getShard(key []byte) ShardDBInterface {
	hash := md5.Sum(key)
	index := binary.BigEndian.Uint32(hash[:4]) % uint32(s.numShards)
	return s.shards[index]
}

// Ghi dữ liệu vào shard tương ứng
func (s *ShardelDB) Put(key, value []byte) error {
	shard := s.getShard(key)
	return shard.Put(key, value)
}

// Đọc dữ liệu từ snapshot thay vì replica
func (s *ShardelDB) Get(key []byte) ([]byte, error) {
	shard := s.getShard(key)
	return shard.Get(key)
}

// Xóa key khỏi shard tương ứng
func (s *ShardelDB) Delete(key []byte) error {
	shard := s.getShard(key)
	return shard.Delete(key)
}

// Đóng toàn bộ database
func (s *ShardelDB) Close() error {
	for _, shard := range s.shards {
		shard.Close()
	}
	return nil
}

// Thêm method Compact để đáp ứng interface ethdb.KeyValueStore
func (s *ShardelDB) Compact(start, limit []byte) error {
	// Implement logic for compacting the database here.  This is a placeholder.
	// You'll need to adapt this based on the requirements of ethdb.KeyValueStore.
	fmt.Println("Compact method called.  Implementation needed.")
	return nil
}

// Thêm method DeleteRange để đáp ứng interface ethdb.KeyValueStore
func (s *ShardelDB) DeleteRange(start, end []byte) error {
	// Implement logic to delete a range of keys from the database here.
	// This is a placeholder. You need to iterate through shards and delete
	// the relevant keys within each shard.  Consider handling potential errors
	// during deletion.
	fmt.Println("DeleteRange method called. Implementation needed.")
	// for _, shard := range s.shards {
	// 	err := shard.DeleteRange(start, end)
	// 	if err != nil {
	// 		return fmt.Errorf("error deleting range from shard: %w", err)
	// 	}
	// }
	return nil
}

func (s *ShardelDB) Has(key []byte) (bool, error) {
	shard := s.getShard(key)
	value, err := shard.Get(key)
	if err != nil {
		return false, err
	}
	return len(value) > 0, nil
}

// NewBatch trả về một batch mới cho ShardelDB
func (s *ShardelDB) NewBatch() ethdb.Batch {
	return &shardBatch{
		s:     s,
		batch: make(map[int][][2][]byte),
	}
}

type shardBatch struct {
	s     *ShardelDB
	batch map[int][][2][]byte
}

func (b *shardBatch) Put(key, value []byte) error {

	shardIndex := int(b.s.getShardIndex(key))
	b.batch[shardIndex] = append(b.batch[shardIndex], [2][]byte{key, value})
	return nil
}

func (b *shardBatch) Delete(key []byte) error {
	shardIndex := int(b.s.getShardIndex(key))
	for i, kv := range b.batch[shardIndex] {
		if bytes.Equal(kv[0], key) {
			b.batch[shardIndex] = append(b.batch[shardIndex][:i], b.batch[shardIndex][i+1:]...)
			return nil
		}
	}
	return nil
}

func (s *ShardelDB) BatchDelete(keys [][]byte) error {
	var g errgroup.Group
	shardKeys := make(map[int][][]byte)
	// Nhóm keys theo shard
	for _, key := range keys {
		shardIndex := int(s.getShardIndex(key))
		shardKeys[shardIndex] = append(shardKeys[shardIndex], key)
	}

	// Xóa song song trên các shard
	for shardIndex, keys := range shardKeys {
		shardIndex, keys := shardIndex, keys // tránh vấn đề closure dùng chung biến
		g.Go(func() error {
			shard := s.shards[shardIndex]
			if shard == nil {
				return fmt.Errorf("shard %d is nil", shardIndex)
			}
			for _, key := range keys {
				if err := shard.Delete(key); err != nil {
					return fmt.Errorf("shard %d: key %x, error: %w", shardIndex, key, err)
				}
			}
			return nil
		})
	}

	// Chờ tất cả goroutine hoàn thành và trả về lỗi (nếu có)
	return g.Wait()
}

func (s *ShardelDB) BatchPut(kvs [][2][]byte) error {
	var g errgroup.Group
	shardBatches := make(map[int][][2][]byte)

	// Lấy block number trước khi dùng logger
	for _, kv := range kvs {
		shardIndex := int(s.getShardIndex(kv[0]))
		shardBatches[shardIndex] = append(shardBatches[shardIndex], kv)
	}

	// Xử lý batch song song theo shard
	for shardIndex, batch := range shardBatches {
		if len(batch) == 0 {
			continue // Bỏ qua batch rỗng
		}
		shardIndex, batch := shardIndex, batch // Tránh closure bug
		g.Go(func() error {
			shard := s.shards[shardIndex]
			if shard == nil {
				return fmt.Errorf("shard %d is nil", shardIndex)
			}
			if err := shard.BatchPut(batch); err != nil {
				return fmt.Errorf("shard %d: batch put error: %w", shardIndex, err)
			}
			return nil
		})
	}

	// Chờ tất cả goroutine hoàn thành và trả về lỗi nếu có
	e := g.Wait()
	return e
}

func (b *shardBatch) Write() error {
	fmt.Println("Write method called. Implementation needed.")
	var wg sync.WaitGroup
	errChan := make(chan error, b.s.numShards)
	for shardIndex, batch := range b.batch {
		wg.Add(1)
		go func(shardIndex int, batch [][2][]byte) {
			defer wg.Done()
			shard := b.s.shards[shardIndex]
			if shard == nil {
				errChan <- fmt.Errorf("shard %d is nil", shardIndex)
				return
			}
			if len(batch) == 0 {
				return
			}
			if err := shard.BatchPut(batch); err != nil {
				errChan <- fmt.Errorf("shard %d: batch: %v, error: %w", shardIndex, batch, err)
			}
		}(shardIndex, batch)
	}

	wg.Wait()
	close(errChan)

	for err := range errChan {
		return err
	}
	return nil
}

func (b *shardBatch) ValueSize() int {
	totalSize := 0
	for _, batch := range b.batch {
		for _, kv := range batch {
			totalSize += len(kv[0]) + len(kv[1])
		}
	}
	return totalSize
}

func (b *shardBatch) Reset() {
	for k := range b.batch {
		b.batch[k] = nil
	}
}

// Replay applies the batch to the database.  **YOU MUST IMPLEMENT THIS**
func (b *shardBatch) Replay(db ethdb.KeyValueWriter) error {
	fmt.Println("Replay method called. Implementation needed.")

	// TODO: Implement Replay logic here. This is a placeholder.
	// You need to iterate through b.batch and apply the Put and Delete operations
	// to the provided db (which is an ethdb.KeyValueWriter).  Consider handling
	// potential errors during the replay process.
	for _, batch := range b.batch {
		for _, kv := range batch {
			if len(kv[1]) == 0 { // Delete operation
				if err := b.Delete(kv[0]); err != nil {
					return fmt.Errorf("error deleting key %x: %w", kv[0], err)
				}
			} else { // Put operation
				if err := b.Put(kv[0], kv[1]); err != nil {
					return fmt.Errorf("error putting key %x: %w", kv[0], err)
				}
			}
		}
	}
	return nil
}

// NewBatchWithSize creates a new batch with a specified size limit.
func (s *ShardelDB) NewBatchWithSize(size int) ethdb.Batch {
	// TODO: Implement NewBatchWithSize logic here. This is a placeholder.
	// You need to create a new shardBatch and potentially set a size limit.
	// Consider how the size limit will be enforced within shardBatch.
	fmt.Println("NewBatchWithSize method called. Implementation needed.")
	return &shardBatch{
		s:     s,
		batch: make(map[int][][2][]byte),
	}
}

// NewIterator returns a new iterator over the database.
func (s *ShardelDB) NewIterator(start, end []byte) ethdb.Iterator {
	// TODO: Implement NewIterator logic here. This is a placeholder.
	// You need to create and return a new iterator that iterates over the data
	// stored in s, starting at the given start key and ending at the given end key.
	// The implementation will depend heavily on how data is stored in ShardelDB.
	fmt.Println("NewIterator method called. Implementation needed.")
	return nil // Placeholder, replace with actual iterator implementation
}

// Thêm hàm Stat() không tham số để đáp ứng interface ethdb.KeyValueStore
func (s *ShardelDB) Stat() (string, error) {
	return fmt.Sprintf("Number of shards: %d", s.numShards), nil
}

// Thêm hàm GetAllKeys vào ShardelDB
func (s *ShardelDB) GetAllKeys() ([]string, error) {
	var allKeys []string
	var mutex sync.Mutex
	var wg sync.WaitGroup

	// Lặp qua tất cả các shard
	for _, shard := range s.shards {
		wg.Add(1)
		go func(shard ShardDBInterface) {
			defer wg.Done()
			// Giả sử mỗi shard có phương thức GetAllKeys() để lấy tất cả các khóa
			keys, err := shard.GetAllKeys() // Bạn cần đảm bảo rằng phương thức này tồn tại trong interface ShardDBInterface
			if err != nil {
				logger.Error("Error getting keys from shard: ", err)
				return
			}
			mutex.Lock()
			allKeys = append(allKeys, keys...) // Thêm các khóa từ shard vào allKeys
			mutex.Unlock()
		}(shard)
	}

	// Chờ tất cả goroutine hoàn thành
	wg.Wait()

	return allKeys, nil
}
