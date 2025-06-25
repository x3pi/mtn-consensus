package storage

// func intMemoryDBIterator() *MemoryDbIterator {
// 	// data := map[string][]byte{
// 	// 	"f1": common.FromHex("f1f1f2"),
// 	// 	"f2": common.FromHex("f2f2f2"),
// 	// }
// 	return nil
// 	// return NewMemoryDbIterator(data)
// }
// func TestNextIterator(t *testing.T) {
// 	iterator := intMemoryDBIterator()

// 	flag := iterator.Next()
// 	assert.Equal(t, flag, true)
// 	assert.Equal(t, iterator.idx, 1)

// 	flag = iterator.Next()
// 	assert.Equal(t, flag, true)
// 	assert.Equal(t, iterator.idx, 2)

// 	flag = iterator.Next()
// 	assert.Equal(t, flag, false)
// }

// func TestKeyIterator(t *testing.T) {
// 	iterator := intMemoryDBIterator()
// 	iterator.Next()
// 	assert.Equal(t, iterator.Key(), common.FromHex(iterator.keys[0]))
// }

// func TestValueIterator(t *testing.T) {
// 	iterator := intMemoryDBIterator()
// 	iterator.Next()
// 	assert.Equal(t, iterator.Value(), iterator.db["f1"])
// }
