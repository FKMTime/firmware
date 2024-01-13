#include "ws_logger.h"

WsLogger::WsLogger() {}
void WsLogger::begin(Stream *_serial, unsigned long _sendInterval = 5000) {
    serial = _serial;
    sendInterval = _sendInterval;
}

void WsLogger::setWsClient(WebSocketsClient *_wsClient) {
    wsClient = _wsClient;
}

void WsLogger::setMaxSize(int logsSize) {
    maxLogsSize = logsSize;
}

void WsLogger::loop() {
    if (millis() - lastSent < sendInterval) return;
    lastSent = millis();

    if (logs.size() == 0 || wsClient == NULL || !wsClient->isConnected()) return;
    JsonDocument logsArrDoc;
    JsonArray arr = logsArrDoc.to<JsonArray>();

    while (logs.size() > 0) {
        logData data = logs.back();
        logs.pop_back();

        JsonObject obj = arr.add<JsonObject>();
        obj["millis"] = data.millis;
        obj["msg"] = data.msg;
    }

    JsonDocument doc;
    doc["logs"]["esp_id"] = ESP.getChipId();
    doc["logs"]["logs"] = logsArrDoc;

    if (wsClient != NULL) {
        String json;
        serializeJson(doc, json);
        wsClient->sendTXT(json);
    }

    logs.clear();
}

void WsLogger::log(String msg) {
    logData data;
    data.millis = millis();
    data.msg = msg;

    logs.push_back(data);
    serial->println(msg);

    if(logs.size() > (unsigned)maxLogsSize) logs.erase(logs.begin());
}

void WsLogger::println(String msg) {
    log(msg);
}

// From: Serial.printf
void WsLogger::printf(const char *format, ...) {
    va_list arg;
    va_start(arg, format);
    char temp[64];
    char* buffer = temp;
    size_t len = vsnprintf(temp, sizeof(temp), format, arg);
    va_end(arg);
    if (len > sizeof(temp) - 1) {
        buffer = new (std::nothrow) char[len + 1];
        if (!buffer) {
            return;
        }
        va_start(arg, format);
        vsnprintf(buffer, len + 1, format, arg);
        va_end(arg);
    }

    log(buffer);
    if (buffer != temp) {
        delete[] buffer;
    }
}

WsLogger Logger;