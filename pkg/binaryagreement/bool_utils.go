package binaryagreement

import "sync"

// BoolSet là một biểu diễn hiệu quả cho một tập hợp các giá trị bool.
type BoolSet uint8

const (
	None  BoolSet = 0b00
	False BoolSet = 0b01
	True  BoolSet = 0b10
	Both  BoolSet = 0b11
)

// Insert thêm một giá trị vào tập hợp, trả về true nếu tập hợp thay đổi.
func (bs *BoolSet) Insert(b bool) bool {
	prev := *bs
	if b {
		*bs |= True
	} else {
		*bs |= False
	}
	return prev != *bs
}

// Contains kiểm tra xem tập hợp có chứa giá trị đã cho không.
func (bs BoolSet) Contains(b bool) bool {
	if b {
		return bs&True != 0
	}
	return bs&False != 0
}

// IsSubset kiểm tra xem tập hợp này có phải là tập con của tập hợp khác không.
func (bs BoolSet) IsSubset(other BoolSet) bool {
	return (bs & other) == bs
}

// Definite trả về giá trị duy nhất trong tập hợp nếu có.
// Trả về (giá trị, true) nếu là tập đơn, ngược lại trả về (false, false).
func (bs BoolSet) Definite() (bool, bool) {
	if bs == True {
		return true, true
	}
	if bs == False {
		return false, true
	}
	return false, false
}

// BoolMultimap ánh xạ một giá trị bool tới một tập hợp các Node ID.
// It is now safe for concurrent use.
type BoolMultimap[N NodeIdT] struct {
	mu       sync.Mutex // Add a mutex to protect the maps
	falseSet map[N]struct{}
	trueSet  map[N]struct{}
}

func NewBoolMultimap[N NodeIdT]() *BoolMultimap[N] {
	return &BoolMultimap[N]{
		falseSet: make(map[N]struct{}),
		trueSet:  make(map[N]struct{}),
	}
}

// Get is not safe to be used concurrently as it exposes the underlying map.
// It is kept here as it was in the original code but should be used with caution.
func (bmm *BoolMultimap[N]) Get(b bool) map[N]struct{} {
	if b {
		return bmm.trueSet
	}
	return bmm.falseSet
}

// Insert thêm một cặp (giá trị, ID) vào map.
// It is now safe to be called from multiple goroutines.
func (bmm *BoolMultimap[N]) Insert(val bool, id N) bool {
	bmm.mu.Lock()         // Lock the mutex before accessing the map
	defer bmm.mu.Unlock() // Defer unlocking until the function returns

	set := bmm.Get(val)
	if _, exists := set[id]; exists {
		return false // Đã tồn tại, không thay đổi
	}
	set[id] = struct{}{}
	return true
}
