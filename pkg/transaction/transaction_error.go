package transaction

import (
	"github.com/ethereum/go-ethereum/common"
	"google.golang.org/protobuf/proto"

	pb "github.com/meta-node-blockchain/meta-node/pkg/mtn_proto"
)

type TransactionError struct {
	Code        int64
	Description string
}

var (
	InvalidTransactionHash              = &TransactionError{1, "invalid transaction hash"}
	InvalidNewDeviceKey                 = &TransactionError{2, "invalid new device key"}
	NotMatchLastHash                    = &TransactionError{3, "not match last hash"}
	InvalidLastDeviceKey                = &TransactionError{4, "invalid last device key"}
	InvalidAmount                       = &TransactionError{5, "invalid amount"}
	InvalidPendingUse                   = &TransactionError{6, "invalid pending use"}
	InvalidDeploySmartContractToAccount = &TransactionError{
		7,
		"invalid deploy smart contract to account",
	}
	InvalidCallSmartContractToAccount = &TransactionError{
		8,
		"invalid call smart contract to  non-existent smart account",
	}
	InvalidCallSmartContractData     = &TransactionError{9, "invalid call smart contract data"}
	InvalidStakeAddress              = &TransactionError{10, "invalid stake address"}
	InvalidUnstakeAddress            = &TransactionError{11, "invalid unstake address"}
	InvalidUnstakeAmount             = &TransactionError{12, "invalid unstake amount"}
	InvalidMaxGas                    = &TransactionError{13, "invalid max gas"}
	InvalidMaxGasPrice               = &TransactionError{14, "invalid max gas price"}
	InvalidCommissionSign            = &TransactionError{15, "invalid commission sign"}
	NotEnoughBalanceForCommissionFee = &TransactionError{
		16,
		"smart contract not enough balance for commission fee",
	}
	InvalidOpenChannelToAccount = &TransactionError{17, "invalid open channel to account"}
	InvalidSign                 = &TransactionError{18, "invalid sign"}
	InvalidCommitAddress        = &TransactionError{19, "invalid commit address"}
	InvalidOpenAccountAmount    = &TransactionError{20, "invalid open account amount"}
	IsNotCreator                = &TransactionError{21, "is not creator"}

	//
	NotEnoughVerifyMinerToVerifyTransation = &TransactionError{
		22,
		"not enough verify miner to verify transaction",
	}
	VerifyTransactionSignTimedOut = &TransactionError{
		23,
		"verify transaction sign timed out",
	}
	InvalidCodeStorage            = &TransactionError{24, "invalid code storage"}
	InvalidCallNonExitAccount     = &TransactionError{25, "invalid call smart contract to non-existent account"}
	InvalidNonce                  = &TransactionError{26, "invalid nonce"}
	InvalidMinTimeUse             = &TransactionError{27, "invalid min time use "}
	TimeoutPending                = &TransactionError{28, "timeout pending"}
	CannotInitializeBLS           = &TransactionError{29, "cannot initialize BLS"} // New error code
	InvalidData                   = &TransactionError{30, "invalid data"}
	AddressMismatch               = &TransactionError{31, "address mismatch for add bls public key"}
	RequiresTwoSignatures         = &TransactionError{32, "transaction requires authentication from 2 signatures"}
	InvalidTransaction            = &TransactionError{33, "invalid transaction"}
	InvalidChainId                = &TransactionError{34, "invalid chain id"}
	InvalidDataInputLengthForTx0  = &TransactionError{35, "invalid data input length for tx0"}
	InvalidBLSSignatureForTx0     = &TransactionError{36, "invalid BLS signature for tx0"}
	FailedToRecoverPubkeyForTx0   = &TransactionError{37, "failed to recover pubkey for tx0"}
	InvalidAddressMatchForTx0     = &TransactionError{38, "invalid address match for tx0"}
	NonceZeroButPublicKeyNotEmpty = &TransactionError{39, "nonce is 0 but public key is not empty"}
	InvalidDeployData             = &TransactionError{40, "invalid deploy data"}
	InvalidCallData               = &TransactionError{41, "invalid call data"}
	InvalidBlockData              = &TransactionError{42, "invalid block data"}
	PublicKeyExists               = &TransactionError{43, "Public key BLS already exists"}
	InvalidSignSecp               = &TransactionError{44, "invalid sign SECP"}

	// Các mã lỗi EXCEPTION mới được thêm vào
	ErrOutOfGas                 = &TransactionError{45, "out of gas"}                 // EXCEPTION_ERR_OUT_OF_GAS = 0
	ErrCodeStoreOutOfGas        = &TransactionError{46, "code store out of gas"}      // EXCEPTION_ERR_CODE_STORE_OUT_OF_GAS = 1
	ErrDepth                    = &TransactionError{47, "depth"}                      // EXCEPTION_ERR_DEPTH = 2
	ErrInsufficientBalance      = &TransactionError{48, "insufficient balance"}       // EXCEPTION_ERR_INSUFFICIENT_BALANCE = 3
	ErrContractAddressCollision = &TransactionError{49, "contract address collision"} // EXCEPTION_ERR_CONTRACT_ADDRESS_COLLISION = 4
	ErrExecutionReverted        = &TransactionError{50, "execution reverted"}         // EXCEPTION_ERR_EXECUTION_REVERTED = 5
	ErrMaxCodeSizeExceeded      = &TransactionError{51, "max code size exceeded"}     // EXCEPTION_ERR_MAX_CODE_SIZE_EXCEEDED = 6
	ErrInvalidJump              = &TransactionError{52, "invalid jump"}               // EXCEPTION_ERR_INVALID_JUMP = 7
	ErrWriteProtection          = &TransactionError{53, "write protection"}           // EXCEPTION_ERR_WRITE_PROTECTION = 8
	ErrReturnDataOutOfBounds    = &TransactionError{54, "return data out of bounds"}  // EXCEPTION_ERR_RETURN_DATA_OUT_OF_BOUNDS = 9
	ErrGasUintOverflow          = &TransactionError{55, "gas uint overflow"}          // EXCEPTION_ERR_GAS_UINT_OVERFLOW = 10
	ErrInvalidCode              = &TransactionError{56, "invalid code"}               // EXCEPTION_ERR_INVALID_CODE = 11
	ErrNonceUintOverflow        = &TransactionError{57, "nonce uint overflow"}        // EXCEPTION_ERR_NONCE_UINT_OVERFLOW = 12
	ErrOutOfBounds              = &TransactionError{58, "out of bounds"}              // EXCEPTION_ERR_OUT_OF_BOUNDS = 13
	ErrOverflow                 = &TransactionError{59, "overflow"}                   // EXCEPTION_ERR_OVERFLOW = 14
	ErrAddressNotInRelated      = &TransactionError{60, "address not in related"}     // EXCEPTION_ERR_ADDRESS_NOT_IN_RELATED = 15
	ErrNone                     = &TransactionError{61, "none"}
)

var CodeToError = map[int64]*TransactionError{
	1:  InvalidTransactionHash,
	2:  InvalidNewDeviceKey,
	3:  NotMatchLastHash,
	4:  InvalidLastDeviceKey,
	5:  InvalidAmount,
	6:  InvalidPendingUse,
	7:  InvalidDeploySmartContractToAccount,
	8:  InvalidCallSmartContractToAccount,
	9:  InvalidCallSmartContractData,
	10: InvalidStakeAddress,
	11: InvalidUnstakeAddress,
	12: InvalidUnstakeAmount,
	13: InvalidMaxGas,
	14: InvalidMaxGasPrice,
	15: InvalidCommissionSign,
	16: NotEnoughBalanceForCommissionFee,
	17: InvalidOpenChannelToAccount,
	18: InvalidSign,
	19: InvalidCommitAddress,
	20: InvalidOpenAccountAmount,
	21: IsNotCreator,
	22: NotEnoughVerifyMinerToVerifyTransation,
	23: VerifyTransactionSignTimedOut,
	24: InvalidCodeStorage,
	25: InvalidCallNonExitAccount,
	26: InvalidNonce,
	27: InvalidMinTimeUse,
	28: TimeoutPending,
	29: CannotInitializeBLS,
	30: InvalidData,
	31: AddressMismatch,
	32: RequiresTwoSignatures,
	33: InvalidTransaction,
	34: InvalidChainId,
	35: InvalidDataInputLengthForTx0,
	36: InvalidBLSSignatureForTx0,
	37: FailedToRecoverPubkeyForTx0,
	38: InvalidAddressMatchForTx0,
	39: NonceZeroButPublicKeyNotEmpty,
	40: InvalidDeployData,
	41: InvalidCallData,
	42: InvalidBlockData,
	43: PublicKeyExists,
	44: InvalidSignSecp,

	// Ánh xạ cho các mã lỗi EXCEPTION mới
	45: ErrOutOfGas,
	46: ErrCodeStoreOutOfGas,
	47: ErrDepth,
	48: ErrInsufficientBalance,
	49: ErrContractAddressCollision,
	50: ErrExecutionReverted,
	51: ErrMaxCodeSizeExceeded,
	52: ErrInvalidJump,
	53: ErrWriteProtection,
	54: ErrReturnDataOutOfBounds,
	55: ErrGasUintOverflow,
	56: ErrInvalidCode,
	57: ErrNonceUintOverflow,
	58: ErrOutOfBounds,
	59: ErrOverflow,
	60: ErrAddressNotInRelated,
	61: ErrNone,
}

func (te *TransactionError) Proto() *pb.TransactionError {
	return &pb.TransactionError{
		Code:        te.Code,
		Description: te.Description,
	}
}

func (te *TransactionError) FromProto(pbData *pb.TransactionError) {
	te.Code = pbData.Code
	te.Description = pbData.Description
}

func (transactionErr *TransactionError) Marshal() ([]byte, error) {
	return proto.Marshal(transactionErr.Proto())
}

func (transactionErr *TransactionError) Unmarshal(data []byte) error {
	pbData := &pb.TransactionError{}
	if err := proto.Unmarshal(data, pbData); err != nil {
		return err
	}
	transactionErr.FromProto(pbData)
	return nil
}

type TransactionHashWithErrorCode struct {
	transactionHash common.Hash
	errorCode       int64
}

func NewTransactionHashWithErrorCode(
	transactionHash common.Hash,
	errorCode int64,
) *TransactionHashWithErrorCode {
	return &TransactionHashWithErrorCode{
		transactionHash: transactionHash,
		errorCode:       errorCode,
	}
}

func (te *TransactionHashWithErrorCode) Proto() *pb.TransactionHashWithErrorCode {
	return &pb.TransactionHashWithErrorCode{
		TransactionHash: te.transactionHash[:],
		Code:            te.errorCode,
	}
}

func (te *TransactionHashWithErrorCode) FromProto(
	pbData *pb.TransactionHashWithErrorCode,
) {
	te.transactionHash = common.BytesToHash(pbData.TransactionHash)
	te.errorCode = pbData.Code
}

func (transactionErr *TransactionHashWithErrorCode) Marshal() ([]byte, error) {
	return proto.Marshal(transactionErr.Proto())
}

func (transactionErr *TransactionHashWithErrorCode) Unmarshal(data []byte) error {
	pbData := &pb.TransactionHashWithErrorCode{}
	if err := proto.Unmarshal(data, pbData); err != nil {
		return err
	}

	transactionErr.FromProto(pbData)
	return nil
}

// MapProtoExceptionToTransactionError ánh xạ một proto.EXCEPTION sang TransactionError tương ứng.
func MapProtoExceptionToTransactionError(exception pb.EXCEPTION) *TransactionError {
	switch exception {
	case pb.EXCEPTION_ERR_OUT_OF_GAS:
		return ErrOutOfGas // Mã 45
	case pb.EXCEPTION_ERR_CODE_STORE_OUT_OF_GAS:
		return ErrCodeStoreOutOfGas // Mã 46
	case pb.EXCEPTION_ERR_DEPTH:
		return ErrDepth // Mã 47
	case pb.EXCEPTION_ERR_INSUFFICIENT_BALANCE:
		return ErrInsufficientBalance // Mã 48
	case pb.EXCEPTION_ERR_CONTRACT_ADDRESS_COLLISION:
		return ErrContractAddressCollision // Mã 49
	case pb.EXCEPTION_ERR_EXECUTION_REVERTED:
		return ErrExecutionReverted // Mã 50
	case pb.EXCEPTION_ERR_MAX_CODE_SIZE_EXCEEDED:
		return ErrMaxCodeSizeExceeded // Mã 51
	case pb.EXCEPTION_ERR_INVALID_JUMP:
		return ErrInvalidJump // Mã 52
	case pb.EXCEPTION_ERR_WRITE_PROTECTION:
		return ErrWriteProtection // Mã 53
	case pb.EXCEPTION_ERR_RETURN_DATA_OUT_OF_BOUNDS:
		return ErrReturnDataOutOfBounds // Mã 54
	case pb.EXCEPTION_ERR_GAS_UINT_OVERFLOW:
		return ErrGasUintOverflow // Mã 55
	case pb.EXCEPTION_ERR_INVALID_CODE:
		return ErrInvalidCode // Mã 56
	case pb.EXCEPTION_ERR_NONCE_UINT_OVERFLOW:
		return ErrNonceUintOverflow // Mã 57
	case pb.EXCEPTION_ERR_OUT_OF_BOUNDS:
		return ErrOutOfBounds // Mã 58
	case pb.EXCEPTION_ERR_OVERFLOW:
		return ErrOverflow // Mã 59
	case pb.EXCEPTION_ERR_ADDRESS_NOT_IN_RELATED:
		return ErrAddressNotInRelated // Mã 60
	case pb.EXCEPTION_NONE:
		return ErrNone // Mã 61
	default:
		// Trả về một lỗi chung hoặc nil nếu không có ánh xạ nào được tìm thấy
		// Ở đây, chúng ta có thể trả về một TransactionError chung cho "unknown exception"
		// hoặc bạn có thể định nghĩa một lỗi cụ thể cho trường hợp này.
		// Ví dụ:
		// return &TransactionError{-2, "unknown or unmapped exception"}
		return nil // Hoặc một lỗi mặc định phù hợp
	}
}
