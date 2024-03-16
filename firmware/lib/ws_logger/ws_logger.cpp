#include "ws_logger.h"

void WsLogger::begin(HardwareSerial* serial, unsigned long _sendInterval) {
    _serial = serial;
    sendInterval = _sendInterval;
}

void WsLogger::setWsClient(WebSocketsClient *_wsClient) {
    wsClient = _wsClient;
}

void WsLogger::setMaxSize(int logsSize) {
    maxLogsSize = logsSize;
}

// not implemented
size_t WsLogger::write(uint8_t val) {
    return 0;
}
size_t WsLogger::write(const uint8_t *buffer, size_t size) {
    logData data;
    data.millis = millis();
    data.msg = String((const char*)buffer);

    logs.push_back(data);
    _serial->write(buffer, size);

    if(logs.size() > (unsigned)maxLogsSize) logs.erase(logs.begin());
    return 0;
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
    doc["logs"]["esp_id"] = ESP.getEfuseMac();
    doc["logs"]["logs"] = logsArrDoc;

    if (wsClient != NULL) {
        String json;
        serializeJson(doc, json);
        wsClient->sendTXT(json);
    }

    logs.clear();
}

WsLogger Logger;