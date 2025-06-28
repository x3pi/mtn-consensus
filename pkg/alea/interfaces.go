// file: pkg/alea/interfaces.go
package alea

// ABA (Asynchronous Binary Agreement) là một interface trừu tượng.
// Bất kỳ triển khai nào cũng phải cung cấp hai phương thức này.
type ABA interface {
	// Propose gửi một đề xuất (0 hoặc 1) cho một vòng thỏa thuận cụ thể.
	Propose(round int, value int)
	// Result trả về một channel chỉ đọc (read-only).
	// Kết quả cuối cùng của vòng thỏa thuận sẽ được gửi qua channel này.
	Result(round int) <-chan int
}

// VCBC (Verifiable Consistent Broadcast) được trừu tượng hóa một phần qua Transport.
// Giao thức Alea cần một cách để gửi tin nhắn đến các node khác.
// Interface Transport sẽ đảm nhiệm việc này.
type Transport interface {
	// ID trả về ID của node hiện tại trong mạng.
	ID() int
	// Broadcast gửi một tin nhắn đến tất cả các node khác trong mạng.
	Broadcast(payload interface{})
	// Events trả về một channel chỉ đọc để nhận các tin nhắn đến.
	Events() <-chan Message
}
