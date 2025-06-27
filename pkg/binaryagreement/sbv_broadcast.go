package binaryagreement

type SbvBroadcast[N NodeIdT] struct {
	netinfo      *NetworkInfo[N]
	binValues    BoolSet
	receivedBval *BoolMultimap[N]
	sentBval     BoolSet
	receivedAux  *BoolMultimap[N]
	terminated   bool
}

func NewSbvBroadcast[N NodeIdT](netinfo *NetworkInfo[N]) *SbvBroadcast[N] {
	return &SbvBroadcast[N]{
		netinfo:      netinfo,
		binValues:    None,
		receivedBval: NewBoolMultimap[N](),
		sentBval:     None,
		receivedAux:  NewBoolMultimap[N](),
		terminated:   false,
	}
}

// Clear reset trạng thái của SBV cho một kỷ nguyên mới.
func (sbv *SbvBroadcast[N]) Clear() {
	sbv.binValues = None
	sbv.receivedBval = NewBoolMultimap[N]()
	sbv.sentBval = None
	sbv.receivedAux = NewBoolMultimap[N]()
	sbv.terminated = false
}

// HandleMessage xử lý các thông điệp BVal và Aux.
func (sbv *SbvBroadcast[N]) HandleMessage(senderId N, msg SbvMessage) (Step[N], error) {
	// time.Sleep(time.Duration(50+rand.Intn(50)) * time.Millisecond)
	if msg.Type == "BVal" {
		return sbv.handleBval(senderId, msg.Value)
	}
	return sbv.handleAux(senderId, msg.Value)
}

func (sbv *SbvBroadcast[N]) sendBval(b bool) (Step[N], error) {
	step := Step[N]{}
	if !sbv.sentBval.Insert(b) {
		return step, nil // Đã gửi rồi, không gửi lại
	}

	step.MessagesToSend = append(step.MessagesToSend, struct {
		Target  Target[N]
		Message Message
	}{
		Target:  Target[N]{ToAll: true},
		Message: Message{Content: SbvMessage{Value: b, Type: "BVal"}},
	})
	return step, nil
}

func (sbv *SbvBroadcast[N]) handleBval(senderId N, b bool) (Step[N], error) {
	step := Step[N]{}
	if !sbv.receivedBval.Insert(b, senderId) {
		step.Faults = append(step.Faults, Fault[N]{Sender: senderId, Kind: FaultDuplicateBVal})
		return step, nil
	}

	countBval := len(sbv.receivedBval.Get(b))

	// Khi nhận đủ 2f+1 BVal(b), thêm b vào binValues và gửi Aux(b) nếu là giá trị đầu tiên.
	if countBval == 2*sbv.netinfo.NumFaulty()+1 {
		if sbv.binValues.Insert(b) {
			if sbv.binValues != Both {
				step.MessagesToSend = append(step.MessagesToSend, struct {
					Target  Target[N]
					Message Message
				}{
					Target:  Target[N]{ToAll: true},
					Message: Message{Content: SbvMessage{Value: b, Type: "Aux"}},
				})
			}
		}
	}

	// Khi nhận đủ f+1 BVal(b), gửi BVal(b) của chính mình.
	if countBval == sbv.netinfo.NumFaulty()+1 {
		bvalStep, _ := sbv.sendBval(b)
		step.MessagesToSend = append(step.MessagesToSend, bvalStep.MessagesToSend...)
	}

	outputStep, _ := sbv.tryOutput()
	step.Output = outputStep.Output
	return step, nil
}

func (sbv *SbvBroadcast[N]) handleAux(senderId N, b bool) (Step[N], error) {
	step := Step[N]{}
	if !sbv.receivedAux.Insert(b, senderId) {
		step.Faults = append(step.Faults, Fault[N]{Sender: senderId, Kind: FaultDuplicateAux})
		return step, nil
	}
	return sbv.tryOutput()
}

// tryOutput kiểm tra xem đã có thể output tập giá trị ứng cử viên chưa.
func (sbv *SbvBroadcast[N]) tryOutput() (Step[N], error) {
	step := Step[N]{}
	if sbv.terminated || sbv.binValues == None {
		return step, nil
	}

	auxCount := 0
	auxVals := None
	if sbv.binValues.Contains(true) {
		count := len(sbv.receivedAux.Get(true))
		if count > 0 {
			auxCount += count
			auxVals.Insert(true)
		}
	}
	if sbv.binValues.Contains(false) {
		count := len(sbv.receivedAux.Get(false))
		if count > 0 {
			auxCount += count
			auxVals.Insert(false)
		}
	}

	if auxCount >= sbv.netinfo.NumCorrect() {
		sbv.terminated = true
		step.Output = auxVals // Trả về tập các giá trị ứng cử viên
	}
	return step, nil
}
