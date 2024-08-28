#ifndef __WEBSOCKET_HPP__
#define __WEBSOCKET_HPP__

#include <ArduinoJson.h>
#include <WebSocketsClient.h>

// OTA
extern bool update;

void initWs();
void parseCardInfoResponse(JsonObject doc);
void parseSolveConfirm(JsonObject doc);
void parseDelegateResponse(JsonObject doc);
void parseDeviceSettings(JsonObject doc);
void parseEpochTime(JsonObject doc);
void parseStartUpdate(JsonObject doc);
void parseApiError(JsonObject doc);
void parseTestPacket(JsonObject doc);
void parseUpdateData(uint8_t *payload, size_t length);
void webSocketEvent(WStype_t type, uint8_t *payload, size_t length);

#endif
