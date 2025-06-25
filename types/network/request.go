package network

type Request interface {
	Message() Message
	Connection() Connection
}
