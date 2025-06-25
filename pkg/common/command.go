package common

const (
	InitConnection                  = "InitConnection"
	GetSmartContractStorage         = "GetSmartContractStorage"
	GetSmartContractStorageResponse = "GetSmartContractStorageResponse"

	GetSmartContractCode         = "GetSmartContractCode"
	GetSmartContractCodeResponse = "GetSmartContractCodeResponse"
	GetSCStorageDBData           = "GetSCStorageDBData"

	GetNodeStateRoot             = "GetNodeStateRoot"
	GetAccountState              = "GetAccountState"
	GetAccountStateWithIdRequest = "GetAccountStateWithIdRequest"
	AccountState                 = "AccountState"
	AccountStateWithIdRequest    = "AccountStateWithIdRequest"

	CancelNodePendingState = "CancelNodePendingState"

	// syncs
	GetNodeSyncData = "GetNodeSyncData"
	NodeSyncData    = "NodeSyncData"

	NodeSyncDataFromNode      = "NodeSyncDataFromNode"
	NodeSyncDataFromValidator = "NodeSyncDataFromValidator"

	GetDeviceKey = "GetDeviceKey"
	DeviceKey    = "DeviceKey"

	// Monitor
	MonitorData = "MonitorData"

	// validator
	SendEvent               = "SendEvent"
	SendPoolTransactons     = "SendPoolTransactons"
	BlockNumber             = "BlockNumber"
	SendVote                = "SendVote"
	GetBlockNumber          = "GetBlockNumber"
	GetTransactionsPool     = "GetTransactionsPool"
	PushFinalizeEvent       = "PushFinalizeEvent"
	ValidatorGetBlockNumber = "ValidatorGetBlockNumber"

	RBC_ECHO       = "RBC_ECHO"
	RBC_VAL        = "RBC_VAL"
	RBC_READY      = "RBC_READY"
	ABA_COIN_SHARE = "ABA_COIN_SHARE"
	ABA_VOTE       = "ABA_VOTE"
	FILL_GAP       = "FILL_GAP"
	FILLER         = "FILLER"
	RBC_GET_BATCH  = "RBC_GET_BATCH"
	RBC_SEND_BATCH = "RBC_SEND_BATCH"
)
