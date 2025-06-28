package consensus

import (
	"sync"
)

type proposal struct {
	round int
	value int
}

// MockABA là một triển khai giả cho ABA.
type MockABA struct {
	numNodes  int
	proposeCh chan proposal
	resultChs map[int]chan int
	mu        sync.Mutex
}

func NewMockABA(numNodes int) *MockABA {
	m := &MockABA{
		numNodes:  numNodes,
		proposeCh: make(chan proposal, numNodes*2),
		resultChs: make(map[int]chan int),
	}
	go m.coordinator()
	return m
}

func (m *MockABA) Propose(round int, value int) {
	m.proposeCh <- proposal{round: round, value: value}
}

func (m *MockABA) Result(round int) <-chan int {
	m.mu.Lock()
	defer m.mu.Unlock()
	if _, ok := m.resultChs[round]; !ok {
		m.resultChs[round] = make(chan int, 1)
	}
	return m.resultChs[round]
}

func (m *MockABA) coordinator() {
	proposals := make(map[int][]int) // round -> list of values

	for p := range m.proposeCh {
		proposals[p.round] = append(proposals[p.round], p.value)

		if len(proposals[p.round]) == m.numNodes {
			decision := 0
			for _, v := range proposals[p.round] {
				if v == 1 {
					decision = 1
					break
				}
			}

			// Lấy channel kết quả và gửi quyết định
			m.mu.Lock()
			if ch, ok := m.resultChs[p.round]; ok {
				ch <- decision
				close(ch) // Đóng channel sau khi gửi
			}
			m.mu.Unlock()
		}
	}
}
