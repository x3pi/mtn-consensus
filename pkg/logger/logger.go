package logger

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"
)

// --- Constants for log levels ---
const (
	FLAG_DEBUGP   = 0
	FLAG_TELEGRAM = 6
	FLAG_TRACE    = 5
	FLAG_DEBUG    = 4
	FLAG_INFO     = 3
	FLAG_WARN     = 2
	FLAG_ERROR    = 1
)

// --- ANSI color codes ---
const (
	Reset  = "\033[0m"
	Red    = "\033[31m"
	Green  = "\033[32m"
	Yellow = "\033[33m"
	Blue   = "\033[34m"
	Purple = "\033[35m"
	Cyan   = "\033[36m"
)

// --- Structs ---
type LoggerConfig struct {
	Flag             int
	Identifier       string
	TelegramToken    string
	TelegramChatId   int
	TelegramThreadId uint
	Outputs          []*os.File
}

type Logger struct {
	Config *LoggerConfig
}

type telegramPayload struct {
	ChatId          string `json:"chat_id"`
	MessageThreadId string `json:"message_thread_id,omitempty"`
	Text            string `json:"text"`
}

// --- Global state ---
var config = &LoggerConfig{
	Flag:    FLAG_INFO,
	Outputs: []*os.File{os.Stdout},
}

var logger = &Logger{Config: config}

// --- Configuration ---
func SetConfig(newConfig *LoggerConfig) { *config = *newConfig; logger.Config = config }
func SetOutputs(outputs []*os.File)     { config.Outputs = outputs }
func SetFlag(flag int)                  { config.Flag = flag }
func SetIdentifier(identifier string)   { config.Identifier = identifier }
func SetTelegramInfo(token string, chatId int) {
	config.TelegramToken, config.TelegramChatId = token, chatId
}
func SetTelegramGroupInfo(token string, chatId int, threadId uint) {
	config.TelegramToken, config.TelegramChatId, config.TelegramThreadId = token, chatId, threadId
}

// --- Public Log API ---
func DebugP(msg interface{}, a ...interface{}) { log(FLAG_DEBUGP, Purple, "DEBUG_P", msg, a...) }
func Trace(msg interface{}, a ...interface{})  { log(FLAG_TRACE, Blue, "TRACE", msg, a...) }
func Debug(msg interface{}, a ...interface{})  { log(FLAG_DEBUG, Cyan, "DEBUG", msg, a...) }
func Info(msg interface{}, a ...interface{})   { log(FLAG_INFO, Green, "INFO", msg, a...) }
func Warn(msg interface{}, a ...interface{})   { log(FLAG_WARN, Yellow, "WARN", msg, a...) }

func Error(msg interface{}, a ...interface{}) {
	if config.Flag < FLAG_ERROR {
		return
	}
	logger.writeToError(formatConsoleLog(Red, "ERROR", msg, a...))
	if config.TelegramToken != "" && config.TelegramChatId != 0 {
		sendToTelegram(formatPlainLog("ERROR", msg, a...))
	}
}

func Telegram(msg interface{}, a ...interface{}) {
	if config.Flag >= FLAG_TELEGRAM {
		sendToTelegram(formatPlainLog("TELE", msg, a...))
	}
}

// --- Internal Logging Logic ---
func log(level int, color, prefix string, msg interface{}, a ...interface{}) {
	if config.Flag >= level {
		logger.writeToOutputs(formatConsoleLog(color, prefix, msg, a...))
	}
}

func (l *Logger) writeToOutputs(buffer []byte) {
	for _, out := range l.Config.Outputs {
		if out != nil {
			out.Write(buffer)
		}
	}
}

func (l *Logger) writeToError(buffer []byte) {
	os.Stderr.Write(buffer)
}

func sendToTelegram(content []byte) {
	payload := telegramPayload{
		ChatId: strconv.Itoa(config.TelegramChatId),
		Text:   string(content),
	}

	if config.TelegramThreadId > 0 {
		payload.MessageThreadId = strconv.FormatUint(uint64(config.TelegramThreadId), 10)
	}

	jsonData, err := json.Marshal(payload)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Marshal Telegram payload lỗi: %v\n", err)
		return
	}

	url := fmt.Sprintf("https://api.telegram.org/bot%s/sendMessage", config.TelegramToken)
	resp, err := http.Post(url, "application/json", bytes.NewBuffer(jsonData))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Gửi Telegram thất bại: %v\n", err)
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		var body bytes.Buffer
		body.ReadFrom(resp.Body)
		fmt.Fprintf(os.Stderr, "Lỗi từ Telegram API: %s - %s\n", resp.Status, body.String())
	}
}

func formatPlainLog(prefix string, msg interface{}, a ...interface{}) []byte {
	var buffer bytes.Buffer
	if config.Identifier != "" {
		buffer.WriteString(fmt.Sprintf("[%s] ", config.Identifier))
	}
	buffer.WriteString(fmt.Sprintf("[%s] ", prefix))

	if str, ok := msg.(string); ok && len(a) > 0 {
		buffer.WriteString(fmt.Sprintf(str, a...))
	} else {
		fmt.Fprint(&buffer, msg)
		for _, item := range a {
			fmt.Fprintf(&buffer, " %v", item)
		}
	}
	return buffer.Bytes()
}

func formatConsoleLog(color, prefix string, msg interface{}, a ...interface{}) []byte {
	var contentBuffer bytes.Buffer
	if config.Identifier != "" {
		fmt.Fprintf(&contentBuffer, "[%s] ", config.Identifier)
	}

	if str, ok := msg.(string); ok && len(a) > 0 {
		fmt.Fprintf(&contentBuffer, str, a...)
	} else {
		fmt.Fprint(&contentBuffer, msg)
		for _, item := range a {
			fmt.Fprintf(&contentBuffer, " %v", item)
		}
	}

	lines := strings.Split(contentBuffer.String(), "\n")
	var buffer bytes.Buffer
	header := fmt.Sprintf(" %s ", time.Now().Format("15:04:05"))
	buffer.WriteString(color)
	fmt.Fprintf(&buffer, "┌─[%s]%s\n", prefix, header)
	for _, line := range lines {
		if strings.TrimSpace(line) != "" {
			fmt.Fprintf(&buffer, "│  %s\n", line)
		}
	}
	buffer.WriteString("└" + strings.Repeat("─", len(prefix)+len(header)+3))
	buffer.WriteString(Reset + "\n")
	return buffer.Bytes()
}
