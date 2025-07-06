package config

// PeerConfig đại diện cho cấu hình của một node ngang hàng.
type PeerConfig struct {
	Id                int    `json:"id"`
	ConnectionAddress string `json:"connection_address"`
	PublicKey         string `json:"public_key"`
}

// ValidatorInfo chứa thông tin về một validator.
type ValidatorInfo struct {
	PublicKey string `json:"public_key"`
}

// NodeConfig là cấu trúc chính chứa toàn bộ cấu hình cho một node.
type NodeConfig struct {
	ID                int             `json:"id"`
	KeyPair           string          `json:"key_pair"`
	Master            PeerConfig      `json:"master"`
	NodeType          string          `json:"node_type"`
	Version           string          `json:"version"`
	ConnectionAddress string          `json:"connection_address"`
	Peers             []PeerConfig    `json:"peers"`
	NumValidator      int             `json:"num_validator"`
	Validator         []ValidatorInfo `json:"validator"`
}
