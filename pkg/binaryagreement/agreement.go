package binaryagreement

import (
	"fmt"

	"github.com/meta-node-blockchain/meta-node/pkg/logger"
)

type BinaryAgreement[N NodeIdT, S SessionIdT] struct {
	netinfo       *NetworkInfo[N]
	sessionId     S
	epoch         uint64
	sbvBroadcast  *SbvBroadcast[N]
	receivedConf  map[N]BoolSet
	confValues    *BoolSet // Các giá trị ứng cử viên từ SBV output
	estimated     *bool
	decision      *bool
	incomingQueue map[uint64]map[N][]MessageContent
}

func NewBinaryAgreement[N NodeIdT, S SessionIdT](netinfo *NetworkInfo[N], sessionId S) *BinaryAgreement[N, S] {
	return &BinaryAgreement[N, S]{
		netinfo:       netinfo,
		sessionId:     sessionId,
		epoch:         0,
		sbvBroadcast:  NewSbvBroadcast[N](netinfo),
		receivedConf:  make(map[N]BoolSet),
		confValues:    nil,
		estimated:     nil,
		decision:      nil,
		incomingQueue: make(map[uint64]map[N][]MessageContent),
	}
}

func (ba *BinaryAgreement[N, S]) Propose(input bool) (Step[N], error) {
	if !(ba.epoch == 0 && ba.estimated == nil) {
		return Step[N]{}, fmt.Errorf("cannot propose at this stage")
	}
	ba.estimated = &input

	msg := SbvMessage{Value: input, Type: "BVal"}
	step, err := ba.sbvBroadcast.HandleMessage(ba.netinfo.OurId(), msg)
	if err != nil {
		return step, err
	}

	step.MessagesToSend = append(step.MessagesToSend, struct {
		Target  Target[N]
		Message Message
	}{
		Target:  Target[N]{ToAll: true},
		Message: Message{Content: msg},
	})

	finalStep, _ := ba.onSbvStep(step)
	for i := range finalStep.MessagesToSend {
		finalStep.MessagesToSend[i].Message.Epoch = ba.epoch
	}
	return finalStep, nil
}

// onSbvStep xử lý kết quả trả về từ SBV
func (ba *BinaryAgreement[N, S]) onSbvStep(sbvStep Step[N]) (Step[N], error) {
	step := sbvStep

	// Nếu SBV đã output và chúng ta chưa xử lý nó
	if sbvStep.Output != nil && ba.confValues == nil {
		if auxVals, ok := sbvStep.Output.(BoolSet); ok {
			ba.confValues = &auxVals
			step.Output = nil // "Tiêu thụ" output này để không bị xử lý lại

			coin := ba.getCoinValue()

			if definiteVal, ok := auxVals.Definite(); ok {
				// Chỉ có một giá trị ứng cử viên
				if definiteVal == coin {
					// Quyết định!
					decideStep := ba.decide(definiteVal)
					step.MessagesToSend = append(step.MessagesToSend, decideStep.MessagesToSend...)
					step.Output = decideStep.Output
				} else {
					// Bắt đầu kỷ nguyên mới với giá trị ứng cử viên
					updateStep, _ := ba.updateEpoch(definiteVal)
					step.MessagesToSend = append(step.MessagesToSend, updateStep.MessagesToSend...)
				}
			} else {
				// Cả hai giá trị là ứng cử viên, bắt đầu kỷ nguyên mới với giá trị của coin
				updateStep, _ := ba.updateEpoch(coin)
				step.MessagesToSend = append(step.MessagesToSend, updateStep.MessagesToSend...)
			}
		}
	}
	return step, nil
}

func (ba *BinaryAgreement[N, S]) HandleMessage(senderId N, msg Message) (Step[N], error) {
	if ba.decision != nil || (msg.Epoch < ba.epoch && msg.CanExpire()) {
		return Step[N]{}, nil
	}

	if msg.Epoch > ba.epoch {
		if _, ok := ba.incomingQueue[msg.Epoch]; !ok {
			ba.incomingQueue[msg.Epoch] = make(map[N][]MessageContent)
		}
		ba.incomingQueue[msg.Epoch][senderId] = append(ba.incomingQueue[msg.Epoch][senderId], msg.Content)
		return Step[N]{}, nil
	}

	if msg.Epoch < ba.epoch {
		return Step[N]{}, nil
	}

	step, err := ba.handleMessageContent(senderId, msg.Content)
	if err != nil {
		return step, err
	}
	// Gán epoch cho các thông điệp gửi đi
	for i := range step.MessagesToSend {
		step.MessagesToSend[i].Message.Epoch = ba.epoch
	}
	return step, nil
}

func (ba *BinaryAgreement[N, S]) handleMessageContent(senderId N, content MessageContent) (Step[N], error) {
	if ba.Terminated() {
		return Step[N]{}, nil
	}

	switch c := content.(type) {
	case SbvMessage:
		sbvStep, err := ba.sbvBroadcast.HandleMessage(senderId, c)
		if err != nil {
			return sbvStep, err
		}
		return ba.onSbvStep(sbvStep)
	case TermMessage:
		if ba.decision == nil {
			return ba.decide(c.Value), nil
		}
		return Step[N]{}, nil
	default:
		return Step[N]{}, fmt.Errorf("message type not implemented yet: %T", c)
	}
}

func (ba *BinaryAgreement[N, S]) decide(value bool) Step[N] {
	step := Step[N]{}
	if ba.decision != nil {
		return step
	}
	fmt.Printf("      ** Node %s DECIDED: %v at epoch %d **\n", ba.netinfo.OurId(), value, ba.epoch)
	ba.decision = &value
	step.Output = value // Gán trực tiếp bool vào Output

	step.MessagesToSend = append(step.MessagesToSend, struct {
		Target  Target[N]
		Message Message
	}{
		Target:  Target[N]{ToAll: true},
		Message: Message{Content: TermMessage{Value: value}},
	})

	return step
}

func (ba *BinaryAgreement[N, S]) updateEpoch(newEstimate bool) (Step[N], error) {
	ba.epoch++
	ba.estimated = &newEstimate
	ba.confValues = nil
	ba.sbvBroadcast.Clear()

	fmt.Printf("      -- Node %s starting new epoch %d with estimate %v --\n", ba.netinfo.OurId(), ba.epoch, newEstimate)

	msg := SbvMessage{Value: newEstimate, Type: "BVal"}
	step, err := ba.sbvBroadcast.HandleMessage(ba.netinfo.OurId(), msg)
	if err != nil {
		return step, err
	}

	step.MessagesToSend = append(step.MessagesToSend, struct {
		Target  Target[N]
		Message Message
	}{
		Target:  Target[N]{ToAll: true},
		Message: Message{Content: msg},
	})

	return step, nil
}

func (ba *BinaryAgreement[N, S]) getCoinValue() bool {
	logger.Error("getCoinValue", ba.epoch%2 == 0)
	// Logic đồng xu đơn giản hóa: epoch chẵn coin=true, epoch lẻ coin=false
	if ba.epoch%2 == 0 {
		return true
	}
	return false
}

func (ba *BinaryAgreement[N, S]) Terminated() bool {
	return ba.decision != nil
}

func (ba *BinaryAgreement[N, S]) GetDecision() (bool, bool) {
	if ba.decision != nil {
		return *ba.decision, true
	}
	return false, false
}

func (ba *BinaryAgreement[N, S]) GetEpoch() uint64 {
	return ba.epoch
}
