package storage

type IIterator interface {
	Next() bool
	Key() []byte
	Value() []byte
	Release()
	Error() error
}
