package binaryagreement

import "sync"

// BoolSet là một tập hợp an toàn cho các giá trị boolean.
type BoolSet struct {
	mu     sync.RWMutex
	values map[bool]struct{}
}

// NewBoolSet tạo một BoolSet mới.
func NewBoolSet() *BoolSet {
	return &BoolSet{
		values: make(map[bool]struct{}),
	}
}

// Insert thêm một giá trị vào tập hợp.
func (bs *BoolSet) Insert(b bool) {
	bs.mu.Lock()
	defer bs.mu.Unlock()
	bs.values[b] = struct{}{}
}

// Contains kiểm tra xem một giá trị có trong tập hợp không.
func (bs *BoolSet) Contains(b bool) bool {
	bs.mu.RLock()
	defer bs.mu.RUnlock()
	_, ok := bs.values[b]
	return ok
}

// Values trả về một slice chứa các giá trị trong tập hợp.
func (bs *BoolSet) Values() []bool {
	bs.mu.RLock()
	defer bs.mu.RUnlock()
	keys := make([]bool, 0, len(bs.values))
	for k := range bs.values {
		keys = append(keys, k)
	}
	return keys
}

// Size trả về số lượng phần tử trong tập hợp.
func (bs *BoolSet) Size() int {
	bs.mu.RLock()
	defer bs.mu.RUnlock()
	return len(bs.values)
}

// IsSubset kiểm tra xem `bs` có phải là một tập hợp con của `other` hay không.
func (bs *BoolSet) IsSubset(other *BoolSet) bool {
	bs.mu.RLock()
	defer bs.mu.RUnlock()
	other.mu.RLock()
	defer other.mu.RUnlock()
	for v := range bs.values {
		if _, ok := other.values[v]; !ok {
			return false
		}
	}
	return true
}

// Copy tạo một bản sao sâu của BoolSet.
// HÀM MỚI CẦN THÊM
func (bs *BoolSet) Copy() *BoolSet {
	bs.mu.RLock()
	defer bs.mu.RUnlock()

	newBs := NewBoolSet()
	for v := range bs.values {
		newBs.values[v] = struct{}{}
	}
	return newBs
}
