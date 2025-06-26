package transaction

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"strings"
	"sync/atomic"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/crypto/secp256k1"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"

	e_types "github.com/ethereum/go-ethereum/core/types"
	"github.com/meta-node-blockchain/meta-node/pkg/bls"
	p_common "github.com/meta-node-blockchain/meta-node/pkg/common"
	"github.com/meta-node-blockchain/meta-node/pkg/logger"
	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
	"github.com/meta-node-blockchain/meta-node/pkg/utils"
	"github.com/meta-node-blockchain/meta-node/types"
)

type Transaction struct {
	proto      *pb.Transaction
	cachedHash atomic.Pointer[common.Hash]
	isDebug    bool
}

// Hàm này sẽ gọi các hàm chuyển đổi cụ thể dựa trên loại giao dịch.
func FromEthTransaction(ethTx *e_types.Transaction, pTx *pb.Transaction) error {
	if ethTx == nil || pTx == nil {
		return errors.New("FromEthTransaction: ethTx hoặc pTx không được rỗng")
	}

	switch ethTx.Type() {
	case e_types.LegacyTxType:
		// Giả định FromEthLegacyTx đã được định nghĩa và có thể truy cập
		// (ví dụ: trong cùng package hoặc import từ package chứa nó)
		return FromEthLegacyTx(ethTx, pTx) //
	case e_types.AccessListTxType:
		// Giả định FromEthEIP2930Tx đã được định nghĩa
		return FromEthEIP2930Tx(ethTx, pTx) //
	case e_types.DynamicFeeTxType:
		// Giả định FromEthEIP1559Tx đã được định nghĩa
		return FromEthEIP1559Tx(ethTx, pTx) //
	// Thêm các case cho BlobTxType (EIP-4844) nếu cần
	// case types.BlobTxType:
	//     return FromEthBlobTx(ethTx, pTx) // Bạn sẽ cần tự định nghĩa hàm này
	default:
		return errors.New("FromEthTransaction: loại giao dịch Ethereum không được hỗ trợ")
	}
}

// NewTransactionFromEth creates a new types.Transaction from an Ethereum e_types.Transaction.
// It determines the transaction type and uses the appropriate conversion function.
func NewTransactionFromEth(ethTx *e_types.Transaction) (types.Transaction, error) {
	if ethTx == nil {
		return nil, errors.New("NewTransactionFromEth: ethTx cannot be nil")
	}

	pTx := &pb.Transaction{}
	var err error
	switch ethTx.Type() {
	case e_types.LegacyTxType:
		err = FromEthLegacyTx(ethTx, pTx)
	case e_types.AccessListTxType:
		err = FromEthEIP2930Tx(ethTx, pTx)
	case e_types.DynamicFeeTxType:
		err = FromEthEIP1559Tx(ethTx, pTx)
	default:
		return nil, errors.New("NewTransactionFromEth: unsupported Ethereum transaction type")
	}

	if err != nil {
		return nil, err
	}
	return TransactionFromProto(pTx), nil
}

// Hàm tổng quát để chuyển đổi pb.Transaction sang types.Transaction của go-ethereum
// Sửa: Kiểu trả về là *types.Transaction (con trỏ) và không dereference NewTx
func (t *Transaction) ToEthTransaction() *e_types.Transaction { // SỬA: Kiểu trả về là con trỏ
	tx := t.proto
	data := []byte{}
	if t.IsCallContract() {
		data = t.CallData().Input()
	}
	if t.IsDeployContract() {
		data = t.DeployData().Code()
	}
	switch tx.Type {
	case e_types.LegacyTxType:
		var toAddress *common.Address
		if len(tx.ToAddress) > 0 {
			addr := common.BytesToAddress(tx.ToAddress)
			toAddress = &addr
		}
		amount := new(big.Int).SetBytes(tx.Amount)
		gasLimit := tx.MaxGas
		gasPrice := new(big.Int).SetUint64(tx.MaxGasPrice)
		var nonceUint64 uint64
		if len(tx.Nonce) > 0 {
			nonceUint64 = new(big.Int).SetBytes(tx.Nonce).Uint64()
		}

		v, r, s := extractSignature(tx)
		innerLegacyTx := &e_types.LegacyTx{
			Nonce:    nonceUint64,
			GasPrice: gasPrice,
			Gas:      gasLimit,
			To:       toAddress,
			Value:    amount,
			Data:     data,
		}
		if v != nil && r != nil && s != nil {
			innerLegacyTx.V = v
			innerLegacyTx.R = r
			innerLegacyTx.S = s
		}
		// SỬA: Trả về con trỏ trực tiếp từ NewTx
		return e_types.NewTx(innerLegacyTx)

	case e_types.AccessListTxType: // EIP-2930
		var toAddress *common.Address
		if len(tx.ToAddress) > 0 {
			addr := common.BytesToAddress(tx.ToAddress)
			toAddress = &addr
		}
		if tx.ChainID == 0 {
			return nil
		} // SỬA: Trả về nil
		chainID := new(big.Int).SetUint64(tx.ChainID)
		amount := new(big.Int).SetBytes(tx.Amount)
		gasLimit := tx.MaxGas
		if tx.MaxGasPrice == 0 {
			return nil
		} // SỬA: Trả về nil
		gasPrice := new(big.Int).SetUint64(tx.MaxGasPrice)
		var nonceUint64 uint64
		if len(tx.Nonce) > 0 {
			nonceUint64 = new(big.Int).SetBytes(tx.Nonce).Uint64()
		}
		accessList := toEthAccessList(tx.AccessList)
		v, r, s := extractSignature(tx)

		innerAccessListTx := &e_types.AccessListTx{
			ChainID:    chainID,
			Nonce:      nonceUint64,
			GasPrice:   gasPrice,
			Gas:        gasLimit,
			To:         toAddress,
			Value:      amount,
			Data:       data,
			AccessList: accessList,
		}
		if v != nil && r != nil && s != nil {
			innerAccessListTx.V = v
			innerAccessListTx.R = r
			innerAccessListTx.S = s
		}
		// SỬA: Trả về con trỏ trực tiếp từ NewTx
		return e_types.NewTx(innerAccessListTx)

	case e_types.DynamicFeeTxType: // EIP-1559
		var toAddress *common.Address
		if len(tx.ToAddress) > 0 {
			addr := common.BytesToAddress(tx.ToAddress)
			toAddress = &addr
		}
		if tx.ChainID == 0 {
			return nil
		}
		chainID := new(big.Int).SetUint64(tx.ChainID)
		amount := new(big.Int).SetBytes(tx.Amount)
		gasLimit := tx.MaxGas
		if tx.GasTipCap == nil || tx.GasFeeCap == nil {
			return nil
		}
		gasTipCap := new(big.Int).SetBytes(tx.GasTipCap)
		gasFeeCap := new(big.Int).SetBytes(tx.GasFeeCap)
		var nonceUint64 uint64
		if len(tx.Nonce) > 0 {
			nonceUint64 = new(big.Int).SetBytes(tx.Nonce).Uint64()
		}
		accessList := toEthAccessList(tx.AccessList)
		v, r, s := extractSignature(tx)

		innerDynamicFeeTx := &e_types.DynamicFeeTx{
			ChainID:    chainID,
			Nonce:      nonceUint64,
			GasTipCap:  gasTipCap,
			GasFeeCap:  gasFeeCap,
			Gas:        gasLimit,
			To:         toAddress,
			Value:      amount,
			Data:       data,
			AccessList: accessList,
		}
		if v != nil && r != nil && s != nil {
			innerDynamicFeeTx.V = v
			innerDynamicFeeTx.R = r
			innerDynamicFeeTx.S = s
		}
		return e_types.NewTx(innerDynamicFeeTx)
	default:
		return nil
	}
}

func NewTransaction(
	fromAddress common.Address,
	toAddress common.Address,
	amount *big.Int,
	maxGas uint64,
	maxGasPrice uint64,
	maxTimeUse uint64,
	data []byte,
	relatedAddresses [][]byte,
	lastDeviceKey common.Hash,
	newDeviceKey common.Hash,
	nonce uint64,
	chainId uint64,
) types.Transaction {
	nonceBytes := make([]byte, 8)
	binary.BigEndian.PutUint64(nonceBytes, nonce)
	proto := &pb.Transaction{
		FromAddress:      fromAddress.Bytes(),
		ToAddress:        toAddress.Bytes(),
		Amount:           amount.Bytes(),
		MaxGas:           maxGas,
		MaxGasPrice:      maxGasPrice,
		MaxTimeUse:       maxTimeUse,
		Data:             data,
		RelatedAddresses: relatedAddresses,
		LastDeviceKey:    lastDeviceKey.Bytes(),
		NewDeviceKey:     newDeviceKey.Bytes(),
		Nonce:            nonceBytes,
		ChainID:          chainId,
	}
	tx := &Transaction{
		proto: proto,
	}
	return tx // Return the *Transaction directly
}

func NewTransactionOffChain(
	lastHash common.Hash,
	fromAddress common.Address,
	toAddress common.Address,
	pendingUse *big.Int,
	amount *big.Int,
	maxGas uint64,
	maxGasPrice uint64,
	maxTimeUse uint64,
	data []byte,
	relatedAddresses [][]byte,
	lastDeviceKey common.Hash,
	newDeviceKey common.Hash,
	nonce uint64,
) types.Transaction {
	nonceBytes := make([]byte, 8)
	binary.BigEndian.PutUint64(nonceBytes, nonce)
	proto := &pb.Transaction{
		FromAddress:      fromAddress.Bytes(),
		ToAddress:        toAddress.Bytes()[:15],
		Amount:           amount.Bytes(),
		MaxGas:           maxGas,
		MaxGasPrice:      maxGasPrice,
		MaxTimeUse:       maxTimeUse,
		Data:             data,
		RelatedAddresses: relatedAddresses,
		LastDeviceKey:    lastDeviceKey.Bytes(),
		NewDeviceKey:     newDeviceKey.Bytes(),
		Nonce:            nonceBytes,
	}
	tx := &Transaction{
		proto: proto,
	}
	return tx // Return the *Transaction directly
}

func (t *Transaction) SetIsDebug(isDebug bool) {
	t.isDebug = isDebug
}

func (t *Transaction) GetIsDebug() bool {
	return t.isDebug
}

func TransactionsToProto(transactions []types.Transaction) []*pb.Transaction {
	rs := make([]*pb.Transaction, len(transactions))
	for i, v := range transactions {
		rs[i] = v.Proto().(*pb.Transaction)
	}
	return rs
}

func TransactionFromProto(txPb *pb.Transaction) types.Transaction {
	return &Transaction{
		proto: txPb,
	}
}

func TransactionsFromProto(pbTxs []*pb.Transaction) []types.Transaction {
	rs := make([]types.Transaction, len(pbTxs))
	for i, v := range pbTxs {
		rs[i] = TransactionFromProto(v)
	}
	return rs
}

func (t *Transaction) Unmarshal(b []byte) error {
	pbTransaction := &pb.Transaction{}
	err := proto.Unmarshal(b, pbTransaction)
	if err != nil {
		return err
	}
	t.FromProto(pbTransaction)
	return nil
}

// Kiểm tra giao dịch có phải là Deploy Contract không
func (t *Transaction) IsDeployContract() bool {
	return t.GetNonce() != 0 && t.FromAddress() != t.ToAddress() && t.ToAddress() == (common.Address{}) && len(t.Data()) > 0
}

// Kiểm tra giao dịch có phải là Call Contract không
func (t *Transaction) IsCallContract() bool {
	return t.ToAddress() != (common.Address{}) && len(t.Data()) > 0
}

// Kiểm tra giao dịch có phải là chuyển tiền thông thường không
func (t *Transaction) IsRegularTransaction() bool {
	return t.ToAddress() != (common.Address{}) && len(t.Data()) == 0
}

func (t *Transaction) Marshal() ([]byte, error) {
	return proto.Marshal(t.proto)
}

func (t *Transaction) Proto() protoreflect.ProtoMessage {
	return t.proto
}

func (t *Transaction) FromProto(pbMessage protoreflect.ProtoMessage) {
	pbTransaction := pbMessage.(*pb.Transaction)
	t.proto = pbTransaction
}

func (t *Transaction) CopyTransaction() types.Transaction {
	// Tạo một bản sao của protobuf message
	newProto := proto.Clone(t.proto).(*pb.Transaction)

	// Tạo một Transaction mới từ bản sao protobuf message
	newTx := &Transaction{
		proto: newProto,
	}

	return newTx
}

func (t *Transaction) String() (str string) {
	if t == nil || t.proto == nil {
		return "Transaction is nil or incomplete"
	}

	defer func() {
		if r := recover(); r != nil {
			str = fmt.Sprintf("Transaction.String() PANIC: %v", r)
		}
	}()

	nonce := t.GetNonce()
	chainId := t.GetChainID()
	txType := t.proto.Type // Lấy Type từ proto

	fromAddress := "nil"
	if t.proto.FromAddress != nil {
		fromAddress = hex.EncodeToString(t.proto.FromAddress)
	}

	toAddress := "nil (Contract Deployment)"
	if t.proto.ToAddress != nil && len(t.proto.ToAddress) > 0 { // Kiểm tra ToAddress không rỗng
		toAddress = hex.EncodeToString(t.proto.ToAddress)
	} else if len(t.proto.ToAddress) == 0 && !t.IsDeployContract() { // Nếu ToAddress rỗng và không phải deploy
		toAddress = "nil"
	}

	amount := big.NewInt(0)
	if t.proto.Amount != nil {
		amount.SetBytes(t.proto.Amount)
	}

	data := ""
	if t.proto.Data != nil {
		data = hex.EncodeToString(t.proto.Data)
	}

	maxGas := t.MaxGas()
	maxGasPrice := t.MaxGasPrice()
	maxTimeUse := t.MaxTimeUse()

	sign := ""
	if t.proto.Sign != nil {
		sign = hex.EncodeToString(t.proto.Sign)
	}

	// Các trường Ethereum bổ sung
	ethR := ""
	if t.proto.R != nil {
		ethR = hex.EncodeToString(t.proto.R)
	}
	ethS := ""
	if t.proto.S != nil {
		ethS = hex.EncodeToString(t.proto.S)
	}
	ethV := ""
	if t.proto.V != nil {
		ethV = hex.EncodeToString(t.proto.V)
	}

	gasTipCap := ""
	if t.proto.GasTipCap != nil {
		gasTipCap = big.NewInt(0).SetBytes(t.proto.GasTipCap).String() + " wei"
	}

	gasFeeCap := ""
	if t.proto.GasFeeCap != nil {
		gasFeeCap = big.NewInt(0).SetBytes(t.proto.GasFeeCap).String() + " wei"
	}

	accessListStr := "nil"
	if len(t.proto.AccessList) > 0 {
		var alBuilder strings.Builder
		alBuilder.WriteString("\n")
		for i, tuple := range t.proto.AccessList {
			alBuilder.WriteString(fmt.Sprintf("    Tuple %d:\n", i))
			alBuilder.WriteString(fmt.Sprintf("      Address: %s\n", common.BytesToAddress(tuple.Address).Hex()))
			alBuilder.WriteString("      StorageKeys: [\n")
			for _, sk := range tuple.StorageKeys {
				alBuilder.WriteString(fmt.Sprintf("        %s\n", common.BytesToHash(sk).Hex()))
			}
			alBuilder.WriteString("      ]\n")
		}
		accessListStr = alBuilder.String()
	}
	// Thêm phần hiển thị related addresses
	relatedAddressesStr := "nil"
	if len(t.proto.RelatedAddresses) > 0 {
		var raBuilder strings.Builder
		raBuilder.WriteString("\n")
		for i, addr := range t.proto.RelatedAddresses {
			raBuilder.WriteString(fmt.Sprintf("    %d: %s\n", i, common.BytesToAddress(addr).Hex()))
		}
		relatedAddressesStr = raBuilder.String()
	}

	str = fmt.Sprintf(`Transaction Details:
  Hash:               %v
  ChainId:            %v
  Nonce:              %d
  Type:               %d
  From:               %s
  To:                 %s
  Value:              %s wei
  Data:               %s
  Gas Limit:          %d
  Gas Price (Legacy): %d
  Max Time Use:       %d
  BLS Signature:      %s
  Related Addresses:  %s
  Ethereum Fields:
    R:                %s
    S:                %s
    V:                %s
  EIP-1559/2930 Fields:
    GasTipCap:        %s
    GasFeeCap:        %s
    AccessList:       %s
`,
		t.Hash(),
		chainId,
		nonce,
		txType, // Hiển thị Type
		fromAddress,
		toAddress,
		amount.String(),
		data,
		maxGas,
		maxGasPrice,
		maxTimeUse,
		sign,
		relatedAddressesStr,
		ethR, // Hiển thị R từ proto
		ethS, // Hiển thị S từ proto
		ethV, // Hiển thị V từ proto
		gasTipCap,
		gasFeeCap,
		accessListStr,
	)

	return str
}

// getter
func (t *Transaction) Hash() common.Hash {
	// Kiểm tra cache có giá trị hay chưa
	if cached := t.cachedHash.Load(); cached != nil {
		return *cached // Trả về giá trị đã cache nếu có
	}

	hashPb := &pb.TransactionHashData{
		FromAddress:   t.proto.FromAddress,
		ToAddress:     t.proto.ToAddress,
		Amount:        t.proto.Amount,
		MaxGas:        t.proto.MaxGas,
		MaxGasPrice:   t.proto.MaxGasPrice,
		MaxTimeUse:    t.proto.MaxTimeUse,
		Data:          t.proto.Data,
		LastDeviceKey: t.proto.LastDeviceKey,
		NewDeviceKey:  t.proto.NewDeviceKey,
		Nonce:         t.proto.Nonce,
		ChainID:       t.proto.ChainID,
		R:             t.proto.R,
		S:             t.proto.S,
		V:             t.proto.V,
		GasTipCap:     t.proto.GasTipCap,
		GasFeeCap:     t.proto.GasFeeCap,
		AccessList:    t.proto.AccessList,
	}
	bHashPb, _ := proto.Marshal(hashPb)

	// Tính giá trị băm
	hash := crypto.Keccak256Hash(bHashPb)

	// Lưu vào cache (atomic)
	t.cachedHash.Store(&hash)

	return hash
}

func (t *Transaction) GetNonce() uint64 {
	if t.proto != nil && len(t.proto.Nonce) == 8 { // Check for valid length
		return binary.BigEndian.Uint64(t.proto.Nonce)
	} else {
		return 0 // Or any default value
	}
}

func (t *Transaction) GetNonce32Bytes() []byte {
	unit64Nonce := t.GetNonce()
	bigIntNonce := big.NewInt(0)
	bigIntNonce.SetUint64(unit64Nonce)
	bNonce := [32]byte{}
	bigIntNonce.FillBytes(bNonce[:])
	return bNonce[:] // Sửa lỗi ở đây: sử dụng bNonce[:] để chuyển đổi array thành slice
}

func (t *Transaction) GetChainID() uint64 {
	return t.proto.ChainID
}

func (t *Transaction) NewDeviceKey() common.Hash {
	return common.BytesToHash(t.proto.NewDeviceKey)
}

func (t *Transaction) LastDeviceKey() common.Hash {
	return common.BytesToHash(t.proto.LastDeviceKey)
}

func (t *Transaction) FromAddress() common.Address {
	return common.BytesToAddress(t.proto.FromAddress)
}

func (t *Transaction) ToAddress() common.Address {
	return common.BytesToAddress(t.proto.ToAddress)
}

func (t *Transaction) Sign() p_common.Sign {
	return p_common.SignFromBytes(t.proto.Sign)
}

func (t *Transaction) Amount() *big.Int {
	return big.NewInt(0).SetBytes(t.proto.Amount)
}

func (t *Transaction) BRelatedAddresses() [][]byte {
	return t.proto.RelatedAddresses
}

func (t *Transaction) UpdateRelatedAddresses(relatedAddresses [][]byte) {
	t.proto.RelatedAddresses = relatedAddresses
}

func (t *Transaction) AddRelatedAddress(address common.Address) {
	// Kiểm tra xem địa chỉ đã tồn tại trong mảng chưa
	for _, existingAddr := range t.proto.RelatedAddresses {
		if bytes.Equal(existingAddr, address.Bytes()) {
			return // Địa chỉ đã tồn tại, không thêm nữa
		}
	}

	// Thêm địa chỉ mới vào mảng
	t.proto.RelatedAddresses = append(t.proto.RelatedAddresses, address.Bytes())
}

func (t *Transaction) UpdateDeriver(LastDeviceKey, NewDeviceKey common.Hash) {
	t.proto.LastDeviceKey = LastDeviceKey.Bytes()
	t.proto.NewDeviceKey = NewDeviceKey.Bytes()
}

func (t *Transaction) SetReadOnly(readOnly bool) {
	t.proto.ReadOnly = readOnly
}

func (t *Transaction) GetReadOnly() bool {
	return t.proto.ReadOnly
}
func (t *Transaction) RelatedAddresses() []common.Address {
	relatedAddresses := make([]common.Address, len(t.proto.RelatedAddresses)+1)
	for i, v := range t.proto.RelatedAddresses {
		relatedAddresses[i] = common.BytesToAddress(v)
	}
	// append to address
	relatedAddresses[len(t.proto.RelatedAddresses)] = t.ToAddress()
	return relatedAddresses
}

func (t *Transaction) Fee(currentGasPrice uint64) *big.Int {
	fee := big.NewInt(int64(t.proto.MaxGas))
	fee = fee.Mul(fee, big.NewInt(int64(currentGasPrice)))
	fee = fee.Mul(fee, big.NewInt(int64((t.proto.MaxTimeUse/1000)+1.0)))
	return fee
}

func (t *Transaction) Data() []byte {
	return t.proto.Data
}

func (t *Transaction) DeployData() types.DeployData {
	deployData := &DeployData{}
	deployData.Unmarshal(t.Data())
	return deployData
}

func (t *Transaction) CallData() types.CallData {
	callData := &CallData{}
	callData.Unmarshal(t.Data())
	return callData
}

func (t *Transaction) GetData() []byte {
	if t.IsCallContract() {
		return t.CallData().Input()
	}
	if t.IsDeployContract() {
		return t.DeployData().Code()
	}
	return t.Data()
}

func (t *Transaction) MaxGas() uint64 {
	return t.proto.MaxGas
}
func (t *Transaction) GasFeeCap() *big.Int {
	return big.NewInt(0).SetBytes(t.proto.GasFeeCap)
}

func (t *Transaction) GasTipCap() *big.Int {
	return big.NewInt(0).SetBytes(t.proto.GasTipCap)
}

func (t *Transaction) MaxGasPrice() uint64 {
	return t.proto.MaxGasPrice
}

func (tx *Transaction) MaxFee() *big.Int {
	maxGas := big.NewInt(0).SetUint64(tx.MaxGas())

	switch tx.proto.Type {
	case 0, 1: // Legacy or EIP-2930
		price := big.NewInt(0).SetUint64(tx.MaxGasPrice())
		return big.NewInt(0).Mul(maxGas, price)

	case 2: // EIP-1559
		feeCap := tx.GasFeeCap()
		return big.NewInt(0).Mul(maxGas, feeCap)

	default:
		fmt.Println("Unknown transaction type.")
		return big.NewInt(0)
	}
}

func (t *Transaction) MaxTimeUse() uint64 {
	return t.proto.MaxTimeUse
}

func (t *Transaction) RawSignatureValues() (v, r, s *big.Int) {
	if t == nil || t.proto == nil {
		return nil, nil, nil
	}

	v = new(big.Int).SetBytes(t.proto.V)
	r = new(big.Int).SetBytes(t.proto.R)
	s = new(big.Int).SetBytes(t.proto.S)

	return v, r, s
}

// setSignatureValues sets the signature values (v, r, s) from *big.Int.
func (t *Transaction) SetSignatureValues(chainID, v, r, s *big.Int) {
	if t == nil {
		return
	}
	t.proto.V = v.Bytes()
	t.proto.R = r.Bytes()
	t.proto.S = s.Bytes()
	// Cập nhật ChainID nếu cần thiết
	t.proto.ChainID = uint64(chainID.Int64())
}

func (t *Transaction) SetSign(privateKey p_common.PrivateKey) {
	t.proto.Sign = bls.Sign(privateKey, t.Hash().Bytes()).Bytes()
}
func (t *Transaction) SetSignBytes(bytes []byte) {
	t.proto.Sign = bytes
}

func (t *Transaction) SetNonce(nonce uint64) {
	nonceBytes := make([]byte, 8)
	binary.BigEndian.PutUint64(nonceBytes, nonce)
	t.proto.Nonce = nonceBytes
}

func (t *Transaction) SetFromAddress(address common.Address) {
	t.proto.FromAddress = address.Bytes()
}

func (t *Transaction) SetToAddress(address common.Address) {
	t.proto.ToAddress = address.Bytes()
}

// validate
func (t *Transaction) ValidEthSign() bool {

	ethTx := t.ToEthTransaction()
	if ethTx == nil {
		return false
	}

	txJson, _ := ethTx.MarshalJSON()

	fmt.Println(string(txJson))
	from, err := DeriveSenderFromEthTransaction(ethTx, utils.Uint64ToBigInt(t.proto.ChainID))
	if err != nil {
		return false
	}

	return from == t.FromAddress()
}

func DeriveSenderFromEthTransaction(tx *e_types.Transaction, chainID *big.Int) (common.Address, error) {
	if tx == nil {
		return common.Address{}, fmt.Errorf("transaction cannot be nil")
	}

	var signer e_types.Signer
	txType := tx.Type()

	switch txType {
	case e_types.LegacyTxType:
		// For legacy transactions, the chainID determines if it's EIP-155 or Homestead/Frontier.
		// If chainID is nil or zero, it's treated as Homestead.
		// Note: ethTx.ChainId() on a legacy tx might be different from the passed chainID
		// if the tx is pre-EIP155 but being processed on an EIP-155 chain.
		// The 'chainID' parameter to this function should be the actual current chain's ID.
		if chainID != nil && chainID.Sign() > 0 && tx.ChainId() != nil && tx.ChainId().Cmp(chainID) == 0 {
			// If the tx has a EIP-155 ChainID that matches the current chain.
			signer = e_types.NewEIP155Signer(chainID)
		} else if tx.ChainId() == nil || tx.ChainId().Sign() == 0 {
			// Pre-EIP155 transaction
			signer = e_types.HomesteadSigner{}
		} else {
			// EIP-155 transaction but its ChainId might not match the provided one,
			// or the provided chainID is zero. Use tx's own chainID for signer if available.
			// This branch handles cases where a tx from another chain (with its own EIP155 ID)
			// is being inspected, or if the passed chainID implies no EIP155 protection
			// but the tx itself is EIP-155.
			// For sender recovery, using the tx's embedded ChainId is correct for EIP155.
			signer = e_types.NewEIP155Signer(tx.ChainId())
		}

	case e_types.AccessListTxType:
		// EIP-2930 (AccessListTx) transactions are EIP-155 protected and require a chainID.
		// The transaction's embedded chain ID should be used.
		if tx.ChainId() == nil || tx.ChainId().Sign() <= 0 {
			return common.Address{}, fmt.Errorf("EIP-2930 transaction is missing a valid chainID for sender recovery")
		}
		// BerlinSigner or LondonSigner are appropriate; LondonSigner is more recent.
		signer = e_types.NewLondonSigner(tx.ChainId())

	case e_types.DynamicFeeTxType:
		// EIP-1559 (DynamicFeeTx) transactions are also EIP-155 protected and require a chainID.
		// The transaction's embedded chain ID should be used.
		if tx.ChainId() == nil || tx.ChainId().Sign() <= 0 {
			return common.Address{}, fmt.Errorf("EIP-1559 transaction is missing a valid chainID for sender recovery")
		}
		signer = e_types.NewLondonSigner(tx.ChainId())

	// case e_types.BlobTxType: // EIP-4844
	// 	if tx.ChainId() == nil || tx.ChainId().Sign() <= 0 {
	// 		return common.Address{}, fmt.Errorf("EIP-4844 transaction is missing a valid chainID for sender recovery")
	// 	}
	// 	// CancunSigner or a specific BlobTxSigner
	// 	signer = e_types.NewCancunSigner(tx.ChainId())

	default:
		return common.Address{}, fmt.Errorf("unsupported transaction type: %d", txType)
	}

	// Recover sender
	sender, err := e_types.Sender(signer, tx)
	if err != nil {
		return common.Address{}, fmt.Errorf("failed to derive sender: %w", err)
	}
	return sender, nil
}

func (t *Transaction) ValidSign(bPub p_common.PublicKey) bool {
	return bls.VerifySign(
		bPub,
		t.Sign(),
		t.Hash().Bytes(),
	)
}

func (t *Transaction) ValidChainID(chainId uint64) bool {
	return t.GetChainID() == chainId
}

func (t *Transaction) ValidNonce(fromAccountState types.AccountState) bool {
	return t.GetNonce() == fromAccountState.Nonce()
}

func (t *Transaction) ValidMinTimeUse() bool {
	return t.MaxTimeUse() >= p_common.MIN_TX_TIME && t.MaxTimeUse() <= p_common.MAX_GROUP_TIME
}

func (t *Transaction) ValidTx0(fromAccountState types.AccountState, chainId string) (bool, int64) {
	if t.GetNonce() == 0 && len(fromAccountState.PublicKeyBls()) == 0 {
		dataInput := t.CallData().Input()

		if len(dataInput) != 113 {
			return false, InvalidDataInputLengthForTx0.Code
		}

		message := append(append([]byte{}, dataInput[:48]...), []byte(chainId)...)
		hash := crypto.Keccak256(message)

		valid := bls.VerifySign(
			p_common.PubkeyFromBytes(dataInput[:48]),
			t.Sign(),
			t.Hash().Bytes(),
		)
		if !valid {
			return false, InvalidBLSSignatureForTx0.Code
		}

		pb, err := secp256k1.RecoverPubkey(hash, dataInput[48:])
		if err != nil {
			return false, FailedToRecoverPubkeyForTx0.Code
		}

		var addr common.Address
		copy(addr[:], crypto.Keccak256(pb[1:])[12:])

		if t.FromAddress() == t.ToAddress() && addr == t.FromAddress() {
			return true, 0
		} else {
			return false, InvalidAddressMatchForTx0.Code
		}
	} else if t.GetNonce() == 0 && len(fromAccountState.PublicKeyBls()) != 0 {
		return false, NonceZeroButPublicKeyNotEmpty.Code
	}

	return true, 0
}

func (t *Transaction) ValidDeviceKey(fromAccountState types.AccountState) bool {
	logger.Info("LastDeviceKey: %v", t.LastDeviceKey())
	return fromAccountState.DeviceKey() == common.Hash{} || // skip check device key if account state doesn't have device key
		crypto.Keccak256Hash(t.LastDeviceKey().Bytes()) == fromAccountState.DeviceKey()
}

func (t *Transaction) ValidMaxGas() bool {
	return t.MaxGas() >= p_common.TRANSFER_GAS_COST
}

func (t *Transaction) ValidMaxGasPrice(currentGasPrice uint64) bool {
	if t.ToAddress() == p_common.NATIVE_SMART_CONTRACT_REWARD_ADDRESS &&
		t.IsCallContract() {
		// skip check gas price for native smart contract
		return true
	}
	return currentGasPrice <= t.MaxGasPrice()
}

func (t *Transaction) ValidAmountSpend(
	fromAccountState types.AccountState,
	spendAmount *big.Int,
) bool {
	totalBalance := big.NewInt(0).Add(fromAccountState.Balance(), fromAccountState.PendingBalance())
	totalSpend := big.NewInt(0).Add(spendAmount, t.Amount())
	return totalBalance.Cmp(totalSpend) >= 0
}

func (t *Transaction) ValidAmount(
	fromAccountState types.AccountState,
) bool {
	fee := t.MaxFee()
	return t.ValidAmountSpend(fromAccountState, fee)
}

func (t *Transaction) ValidPendingUse(fromAccountState types.AccountState) bool {
	pendingBalance := fromAccountState.PendingBalance()
	pendingUse := fromAccountState.PendingBalance()
	return pendingUse.Cmp(pendingBalance) <= 0
}

func (t *Transaction) ValidDeploySmartContractToAccount(fromAccountState types.AccountState) bool {

	validToAddress := crypto.CreateAddress(fromAccountState.Address(), fromAccountState.Nonce())

	if validToAddress != t.ToAddress() {
		logger.Warn("Not match deploy address", validToAddress, t.ToAddress())
	}
	return validToAddress == t.ToAddress()
}

func (t *Transaction) ValidDeployData() bool {
	if t.IsDeployContract() {
		if t.DeployData() == nil || len(t.DeployData().Code()) == 0 {
			logger.Warn("Deploy data is nil")
			return false
		}
	}
	return true
}

func (t *Transaction) ValidCallData() bool {
	if t.IsCallContract() {

		if t.CallData() == nil || len(t.CallData().Input()) == 0 {
			logger.Warn("Deploy data is nil")
			return false
		}
	}
	return true
}

func (t *Transaction) ValidOpenChannelToAccount(fromAccountState types.AccountState) bool {
	// validToAddress := common.BytesToAddress(
	// 	crypto.Keccak256(
	// 		append(
	// 			fromAccountState.Address().Bytes(),
	// 			fromAccountState.LastHash().Bytes()...),
	// 	)[12:],
	// )
	validToAddress := crypto.CreateAddress(fromAccountState.Address(), fromAccountState.Nonce())

	if validToAddress != t.ToAddress() {
		logger.Warn("Not match open channel address", validToAddress, t.ToAddress())
	}
	return validToAddress == t.ToAddress()
}

func (t *Transaction) ValidCallSmartContractToAccount(toAccountState types.AccountState) bool {
	if t.IsCallContract() {
		scState := toAccountState.SmartContractState()
		return scState != nil
	}
	return true
}

func MarshalTransactions(txs []types.Transaction) ([]byte, error) {
	return proto.Marshal(&pb.Transactions{Transactions: TransactionsToProto(txs)})
}

func UnmarshalTransactions(b []byte) ([]types.Transaction, error) {
	pbTxs := &pb.Transactions{}
	err := proto.Unmarshal(b, pbTxs)
	if err != nil {
		return nil, err
	}
	return TransactionsFromProto(pbTxs.Transactions), nil
}

func MarshalTransactionsWithBlockNumber(
	txs []types.Transaction,
	blockNumber uint64,
) ([]byte, error) {
	pbTxs := make([]*pb.Transaction, len(txs))
	for i, v := range txs {
		pbTxs[i] = v.Proto().(*pb.Transaction)
	}
	return proto.Marshal(&pb.TransactionsWithBlockNumber{
		Transactions: pbTxs,
		BlockNumber:  blockNumber,
	})
}

func UnmarshalTransactionsWithBlockNumber(b []byte) ([]types.Transaction, uint64, error) {
	pbTxs := &pb.TransactionsWithBlockNumber{}
	err := proto.Unmarshal(b, pbTxs)
	if err != nil {
		return nil, 0, err
	}
	rs := make([]types.Transaction, len(pbTxs.Transactions))
	for i, v := range pbTxs.Transactions {
		rs[i] = &Transaction{proto: v}
	}
	return rs, pbTxs.BlockNumber, nil
}

func (t *Transaction) MarshalJSON() ([]byte, error) {
	// Tạo một map để lưu trữ dữ liệu của transaction
	data := map[string]interface{}{
		"Hash":             hex.EncodeToString(t.Hash().Bytes()),
		"FromAddress":      hex.EncodeToString(t.proto.FromAddress),
		"ToAddress":        hex.EncodeToString(t.proto.ToAddress),
		"Amount":           hex.EncodeToString(t.proto.Amount),
		"MaxGas":           t.proto.MaxGas,
		"MaxGasPrice":      t.proto.MaxGasPrice,
		"MaxTimeUse":       t.proto.MaxTimeUse,
		"Data":             hex.EncodeToString(t.proto.Data),
		"RelatedAddresses": t.proto.RelatedAddresses,
		"LastDeviceKey":    hex.EncodeToString(t.proto.LastDeviceKey),
		"NewDeviceKey":     hex.EncodeToString(t.proto.NewDeviceKey),
		"Nonce":            t.GetNonce(),
		"Sign":             hex.EncodeToString(t.proto.Sign),
		"ReadOnly":         t.GetReadOnly(),
	}

	// Chuyển đổi map thành JSON
	return json.Marshal(data)
}

func (t *Transaction) UnmarshalJSON(data []byte) error {
	// Tạo một map để lưu trữ dữ liệu JSON
	var jsonData map[string]interface{}
	err := json.Unmarshal(data, &jsonData)
	if err != nil {
		return err
	}

	// Lấy dữ liệu từ map và gán cho các trường của transaction
	// ... (Cần thêm logic để chuyển đổi dữ liệu từ JSON về các kiểu dữ liệu tương ứng của Go) ...

	return nil
}

func prepareTransactionSpecificData(
	ethTx *e_types.Transaction,
	storageAddressForDeploy common.Address,
) (bData []byte, err error) {

	txData := ethTx.Data()
	ethRecipient := ethTx.To() // This is *common.Address

	if len(txData) > 0 && ethRecipient == nil {
		// Case 1: Contract Deployment
		// For contract creation, 'toAddress' is conventionally the zero address,
		// but your NewTransaction might expect common.Address{} which is fine.
		// The actual contract address is computed later from sender and nonce.
		// Here, we set toAddress to indicate it's a creation, or let NewTransaction handle it.
		// For clarity, if NewTransaction expects a zero address for its 'to' field in deploys:

		// Using NewDeployData from the current 'transaction' package
		deployData := NewDeployData(txData, storageAddressForDeploy)
		bData, err = deployData.Marshal()
		if err != nil {
			return nil, fmt.Errorf("error marshalling DeployData: %w", err)
		}

	} else if len(txData) > 0 && ethRecipient != nil {
		// Case 2: Contract Call

		// Using NewCallData from the current 'transaction' package
		callData := NewCallData(txData)
		bData, err = callData.Marshal()
		if err != nil {
			// Changed from panic(err) in original commented code
			return nil, fmt.Errorf("error marshalling CallData: %w", err)
		}

	} else if len(txData) == 0 && ethRecipient != nil {
		// Case 3: Simple Transfer (or transaction with no data to a specific account)
		bData = nil // No specific data (DeployData/CallData) to marshal

	} else if len(txData) == 0 && ethRecipient == nil {

		bData = nil
	}

	return bData, nil
}
