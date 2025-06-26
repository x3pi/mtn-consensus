package binaryagreement

import (
	"errors"
	"fmt"
	"sync"
)

// NetworkInfo chứa thông tin về mạng.
type NetworkInfo struct {
	N int // Tổng số nút
	F int // Số nút lỗi tối đa
}

// QuorumSize trả về số lượng nút cần thiết cho một quorum thông thường.
func (ni *NetworkInfo) QuorumSize() int { return ni.F + 1 }

// ByzantineQuorumSize trả về số lượng nút cần thiết cho một Byzantine quorum.
func (ni *NetworkInfo) ByzantineQuorumSize() int { return 2*ni.F + 1 }

// CorrectQuorumSize trả về số lượng nút trung thực tối thiểu.
func (ni *NetworkInfo) CorrectQuorumSize() int { return ni.N - ni.F }

// MoustefaouiABA là triển khai của thuật toán ABA.
type MoustefaouiABA struct {
	mu sync.Mutex

	pid       string
	replicaID int
	netInfo   *NetworkInfo
	sigLib    ThreshSigLib

	round    int64
	decision *bool

	// State per round
	estimate      map[int64]*bool
	receivedBVal  map[int64]map[bool]map[int]struct{}
	receivedAux   map[int64]map[bool]map[int]struct{}
	receivedConf  map[int64]map[int]*BoolSet
	receivedTerm  map[bool]map[int]struct{}
	binValues     map[int64]*BoolSet
	sentBVal      map[int64]*BoolSet
	coin          map[int64]*Coin
	sbvTerminated map[int64]bool

	// SỬA LỖI QUAN TRỌNG: Thêm confValues để "đóng băng" trạng thái binValues
	// trước khi ra quyết định cho vòng tiếp theo.
	confValues map[int64]*BoolSet
}

// NewMoustefaouiABA tạo một thể hiện ABA mới.
func NewMoustefaouiABA(pid string, replicaID int, netInfo *NetworkInfo, sigLib ThreshSigLib) *MoustefaouiABA {
	aba := &MoustefaouiABA{
		pid:           pid,
		replicaID:     replicaID,
		netInfo:       netInfo,
		sigLib:        sigLib,
		round:         0,
		estimate:      make(map[int64]*bool),
		receivedBVal:  make(map[int64]map[bool]map[int]struct{}),
		receivedAux:   make(map[int64]map[bool]map[int]struct{}),
		receivedConf:  make(map[int64]map[int]*BoolSet),
		receivedTerm:  make(map[bool]map[int]struct{}),
		binValues:     make(map[int64]*BoolSet),
		sentBVal:      make(map[int64]*BoolSet),
		coin:          make(map[int64]*Coin),
		sbvTerminated: make(map[int64]bool),
		confValues:    make(map[int64]*BoolSet), // Khởi tạo confValues
	}
	aba.initRound(0)
	return aba
}

// initRound khởi tạo tất cả các cấu trúc dữ liệu cần thiết cho một vòng mới.
func (aba *MoustefaouiABA) initRound(r int64) {
	if _, ok := aba.receivedBVal[r]; !ok {
		aba.receivedBVal[r] = map[bool]map[int]struct{}{true: {}, false: {}}
		aba.receivedAux[r] = map[bool]map[int]struct{}{true: {}, false: {}}
		aba.receivedConf[r] = make(map[int]*BoolSet)
		aba.binValues[r] = NewBoolSet()
		aba.sentBVal[r] = NewBoolSet()
		// không cần khởi tạo confValues[r] ở đây, nó sẽ được tạo khi cần.
		aba.coin[r] = NewCoin(aba.pid, r, aba.sigLib, aba.netInfo.ByzantineQuorumSize())
	}
}

// HandleInput xử lý giá trị đề xuất ban đầu.
func (aba *MoustefaouiABA) HandleInput(value bool) (*Step, error) {
	aba.mu.Lock()
	defer aba.mu.Unlock()

	if aba.round != 0 || aba.estimate[0] != nil {
		return &Step{}, errors.New("input can only be handled at round 0 before any estimate is set")
	}
	aba.estimate[0] = &value
	bvalMsg := BValMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: 0}, Value: value}
	step, err := aba.handleBVal(bvalMsg)
	if err != nil {
		return &Step{}, err
	}
	step.Messages = append(step.Messages, bvalMsg)
	step.ToBroadcast = true
	return step, nil
}

// HandleMessage xử lý thông điệp đến.
func (aba *MoustefaouiABA) HandleMessage(msg Message) (*Step, error) {
	aba.mu.Lock()
	defer aba.mu.Unlock()

	if aba.HasTerminated() {
		return &Step{}, nil
	}
	if m, ok := msg.(TermMessage); ok {
		return aba.handleTerm(m)
	}
	if msg.GetRound() < aba.round {
		return &Step{}, nil
	}
	aba.initRound(msg.GetRound())

	switch m := msg.(type) {
	case BValMessage:
		return aba.handleBVal(m)
	case AuxMessage:
		return aba.handleAux(m)
	case ConfMessage:
		return aba.handleConf(m)
	case CoinMessage:
		return aba.handleCoin(m)
	default:
		return nil, fmt.Errorf("unknown message type: %T", msg)
	}
}

// handleBVal xử lý thông điệp BVal.
func (aba *MoustefaouiABA) handleBVal(msg BValMessage) (*Step, error) {
	r, v, s := msg.Round, msg.Value, msg.Sender
	step := &Step{}

	if _, exists := aba.receivedBVal[r][v][s]; exists {
		return step, nil
	}
	aba.receivedBVal[r][v][s] = struct{}{}

	if len(aba.receivedBVal[r][v]) == aba.netInfo.QuorumSize() && !aba.sentBVal[r].Contains(v) {
		aba.sentBVal[r].Insert(v)
		bvalMsg := BValMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: r}, Value: v}
		step.Messages = append(step.Messages, bvalMsg)
		step.ToBroadcast = true
		if _, err := aba.handleBVal(bvalMsg); err != nil {
			return nil, fmt.Errorf("error self-handling bval echo: %w", err)
		}
	}

	if len(aba.receivedBVal[r][v]) == aba.netInfo.ByzantineQuorumSize() && !aba.binValues[r].Contains(v) {
		aba.binValues[r].Insert(v)
		auxMsg := AuxMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: r}, Value: v}
		step.Messages = append(step.Messages, auxMsg)
		step.ToBroadcast = true
		if _, err := aba.handleAux(auxMsg); err != nil {
			return nil, fmt.Errorf("error self-handling aux message: %w", err)
		}
	}

	sbvStep, err := aba.tryOutputSbv(r)
	if err != nil {
		return nil, err
	}
	step.Messages = append(step.Messages, sbvStep.Messages...)
	if sbvStep.ToBroadcast {
		step.ToBroadcast = true
	}
	return step, nil
}

// handleAux xử lý thông điệp Aux.
func (aba *MoustefaouiABA) handleAux(msg AuxMessage) (*Step, error) {
	r, v, s := msg.Round, msg.Value, msg.Sender
	if _, exists := aba.receivedAux[r][v][s]; exists {
		return &Step{}, nil
	}
	aba.receivedAux[r][v][s] = struct{}{}
	return aba.tryOutputSbv(r)
}

// tryOutputSbv cố gắng kết thúc giai đoạn con "Validated Byzantine Broadcast" (SBV) của vòng.
func (aba *MoustefaouiABA) tryOutputSbv(r int64) (*Step, error) {
	if aba.sbvTerminated[r] {
		return &Step{}, nil
	}

	senders := make(map[int]struct{})
	for _, v := range aba.binValues[r].Values() {
		for senderID := range aba.receivedAux[r][v] {
			senders[senderID] = struct{}{}
		}
	}

	if len(senders) >= aba.netInfo.CorrectQuorumSize() {
		aba.sbvTerminated[r] = true

		// SỬA LỖI QUAN TRỌNG: Chốt lại giá trị confValues tại đây.
		if _, ok := aba.confValues[r]; !ok {
			aba.confValues[r] = aba.binValues[r].Copy()
		}

		// Gửi Conf với giá trị đã được chốt.
		confMsg := ConfMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: r}, Value: aba.confValues[r]}
		step := &Step{Messages: []Message{confMsg}, ToBroadcast: true}

		if _, err := aba.handleConf(confMsg); err != nil {
			return nil, fmt.Errorf("error self-handling conf message in tryOutputSbv: %w", err)
		}
		return step, nil
	}
	return &Step{}, nil
}

// handleConf xử lý thông điệp Conf.
// handleConf xử lý thông điệp Conf.
func (aba *MoustefaouiABA) handleConf(msg ConfMessage) (*Step, error) {
	r, s := msg.Round, msg.Sender
	step := &Step{}

	if _, exists := aba.receivedConf[r][s]; exists {
		return step, nil
	}
	aba.receivedConf[r][s] = msg.Value

	// SỬA LỖI QUAN TRỌNG: Xác thực CONF đến dựa trên binValues (linh hoạt hơn)
	// thay vì confValues (đã đóng băng và quá nghiêm ngặt).
	// Điều này cho phép các nút chấp nhận bằng chứng từ những nút có góc nhìn hơi khác
	// nhưng vẫn hợp lệ, giúp phá vỡ deadlock trong kịch bản Byzantine.
	myBinValues := aba.binValues[r]

	confCount := 0
	for _, peerConfValues := range aba.receivedConf[r] {
		if peerConfValues.IsSubset(myBinValues) {
			confCount++
		}
	}

	// Nếu có đủ CONF hợp lệ VÀ giai đoạn SBV của chính mình đã kết thúc
	if confCount >= aba.netInfo.CorrectQuorumSize() && aba.sbvTerminated[r] {
		// Chỉ kích hoạt đồng xu một lần duy nhất
		if aba.coin[r].StartOnce() {
			share := aba.coin[r].GetMyShare()
			coinMsg := CoinMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: r}, Share: share}
			step.Messages = append(step.Messages, coinMsg)
			step.ToBroadcast = true
			if _, err := aba.handleCoin(coinMsg); err != nil {
				return nil, fmt.Errorf("error self-handling coin message: %w", err)
			}
		}
	}

	// Luôn thử cập nhật vòng, phòng trường hợp đồng xu đã quyết định trước đó.
	updateStep, err := aba.tryUpdateRound(r)
	if err != nil {
		return nil, err
	}
	step.Messages = append(step.Messages, updateStep.Messages...)
	if updateStep.ToBroadcast {
		step.ToBroadcast = true
	}

	return step, nil
}

// handleCoin xử lý thông điệp Coin.
func (aba *MoustefaouiABA) handleCoin(msg CoinMessage) (*Step, error) {
	r, s := msg.Round, msg.Sender
	coin := aba.coin[r]

	if coin.HasDecided() {
		return &Step{}, nil
	}
	coin.AddShare(s, msg.Share)

	if coin.HasDecided() {
		return aba.tryUpdateRound(r)
	}
	return &Step{}, nil
}
func (aba *MoustefaouiABA) handleTerm(msg TermMessage) (*Step, error) {
	v, s := msg.Value, msg.Sender
	step := &Step{}

	// Lấy vòng hiện tại của nút nhận. Đây là thay đổi cốt lõi.
	currentRound := aba.round

	if _, ok := aba.receivedTerm[v]; !ok {
		aba.receivedTerm[v] = make(map[int]struct{})
	}
	if _, exists := aba.receivedTerm[v][s]; exists {
		return &Step{}, nil // Đã xử lý, bỏ qua
	}
	aba.receivedTerm[v][s] = struct{}{}

	// Điều kiện kết thúc nhanh: nhận được f+1 TERM(v)
	if len(aba.receivedTerm[v]) >= aba.netInfo.QuorumSize() {
		return aba.decide(v)
	}

	// SỬA LỖI CUỐI CÙNG: Coi TermMessage là bằng chứng cho VÒNG HIỆN TẠI của người nhận.
	// Điều này đảm bảo thông tin không bao giờ bị coi là "lỗi thời".

	// 1. Coi như đã nhận BVAL(v, currentRound) từ s
	bvalStep, err := aba.handleBVal(BValMessage{BaseMessage: BaseMessage{Sender: s, Round: currentRound}, Value: v})
	if err != nil {
		return nil, fmt.Errorf("error handling Term as BVAL: %w", err)
	}
	step.Messages = append(step.Messages, bvalStep.Messages...)
	if bvalStep.ToBroadcast {
		step.ToBroadcast = true
	}

	// 2. Coi như đã nhận AUX(v, currentRound) từ s
	auxStep, err := aba.handleAux(AuxMessage{BaseMessage: BaseMessage{Sender: s, Round: currentRound}, Value: v})
	if err != nil {
		return nil, fmt.Errorf("error handling Term as AUX: %w", err)
	}
	step.Messages = append(step.Messages, auxStep.Messages...)
	if auxStep.ToBroadcast {
		step.ToBroadcast = true
	}

	return step, nil
}

// tryUpdateRound cố gắng chuyển sang vòng mới.
func (aba *MoustefaouiABA) tryUpdateRound(r int64) (*Step, error) {
	coin := aba.coin[r]
	confVals, confOk := aba.confValues[r]

	if r != aba.round || !coin.HasDecided() || !confOk {
		return &Step{}, nil
	}

	coinVal, _ := coin.Value()
	var nextEstimate bool

	if confVals.Size() == 1 && confVals.Contains(coinVal) {
		return aba.decide(coinVal)
	} else if confVals.Size() == 1 {
		vals := confVals.Values()
		nextEstimate = vals[0]
	} else {
		nextEstimate = coinVal
	}

	aba.round++
	nextRound := aba.round
	fmt.Printf("🚀 [Node %d] ADVANCING to round %d with estimate %v\n", aba.replicaID, nextRound, nextEstimate)

	aba.initRound(nextRound)
	aba.estimate[nextRound] = &nextEstimate

	bvalMsg := BValMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: nextRound}, Value: nextEstimate}
	step := &Step{Messages: []Message{bvalMsg}, ToBroadcast: true}

	if _, err := aba.handleBVal(bvalMsg); err != nil {
		return nil, fmt.Errorf("error self-handling bval for new round: %w", err)
	}
	return step, nil
}

// decide quyết định một giá trị cuối cùng.
func (aba *MoustefaouiABA) decide(value bool) (*Step, error) {
	if aba.decision != nil {
		return &Step{}, nil
	}
	aba.decision = &value
	termMsg := TermMessage{BaseMessage: BaseMessage{Sender: aba.replicaID, Round: aba.round}, Value: value}
	return &Step{Decision: &value, Messages: []Message{termMsg}, ToBroadcast: true}, nil
}

func (aba *MoustefaouiABA) HasTerminated() bool {
	return aba.decision != nil
}

func (aba *MoustefaouiABA) Deliver() (bool, bool) {
	if aba.decision != nil {
		return *aba.decision, true
	}
	return false, false
}
