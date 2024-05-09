#ifndef __WEBSOCKET_HPP__
#define __WEBSOCKET_HPP__

#include "lcd.hpp"
#include "globals.hpp"
#include <ws_logger.h>
#include "version.h"
#include "defines.h"
#include "radio/utils.hpp"

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length);
String wsURL = "";

// OTA
int sketchSize = 0;
int sketchSizeRemaining = 0;
bool update = false;

void initWs() {
  lcdPrintf(0, true, ALIGN_CENTER, "FKM");
  lcdPrintf(1, true, ALIGN_CENTER, "Looking for MDNS");

  while (true) {
    wsURL = getWsUrl();
    if (wsURL.length() > 0)
      break;
    delay(1000);
  }

  ws_info_t wsInfo = parseWsUrl(wsURL.c_str());

  char finalPath[256];
  snprintf(finalPath, 256, "%s?id=%lu&ver=%s&chip=%s&bt=%s&firmware=%s", 
            wsInfo.path, getEspId(), FIRMWARE_VERSION, CHIP, BUILD_TIME, FIRMWARE_TYPE);

  webSocket.begin(wsInfo.host, wsInfo.port, finalPath);
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(1500);
  Logger.setWsClient(&webSocket);
}

typedef ArduinoJson::V701PB2::detail::MemberProxy<ArduinoJson::V701PB2::JsonDocument &, const char *> JsonChildDocument;
void parseCardInfoResponse(JsonChildDocument doc) {
  String display = doc["display"];
  unsigned long cardId = doc["card_id"];
  String countryIso2 = doc["country_iso2"];
  bool canCompete = doc["can_compete"];
  countryIso2.toLowerCase();

  if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR || state.currentScene == SCENE_WAITING_FOR_COMPETITOR_WITH_TIME) {
    if(!state.testMode && (!webSocket.isConnected() || !stackmat.connected())) return;

    if (state.competitorCardId == 0 && canCompete) {
      strncpy(state.competitorDisplay, display.c_str(), 128);
      state.competitorCardId = cardId;
      primaryLangauge = countryIso2 != "pl";
      state.currentScene = SCENE_COMPETITOR_INFO;

      if (state.solveTime != stackmat.time()) {
        if (state.lastTimerState == ST_Stopped) {
          startSolveSession(stackmat.time());
        } else {
          if (state.useInspection) stopInspection();

          state.currentScene = SCENE_TIMER_TIME;
        }
      }
    }
  } else if (state.currentScene == SCENE_FINISHED_TIME) {
    if (state.competitorCardId != cardId && state.timeConfirmed) {
      state.judgeCardId = cardId;
    } else if(state.judgeCardId > 0 && state.competitorCardId == cardId) {
      sendSolve(false);
    }
  }

  stateHasChanged = true;
}

void parseSolveConfirm(JsonChildDocument doc) {
  if (doc["competitor_id"] != state.competitorCardId ||
      doc["esp_id"] != getEspId() ||
      doc["session_id"] != state.solveSessionId) {
    Logger.println("Wrong solve confirm frame!");
    return;
  }

  resetSolveState();
}

void parseDelegateResponse(JsonChildDocument doc) {
  if (doc["esp_id"] != getEspId()) {
    Logger.println("Wrong solve confirm frame!");
    return;
  }

  if (doc.containsKey("solve_time")) {
    unsigned long solveTime =  doc["solve_time"];
    state.solveTime = solveTime;
    state.lastSolveTime = solveTime;
  }

  if (doc.containsKey("penalty")) {
    int penalty =  doc["penalty"];
    state.penalty = penalty;
  }
  
  bool shouldScanCards =  doc["should_scan_cards"];
  state.timeConfirmed = true;

  if(shouldScanCards) {
    state.currentScene = SCENE_FINISHED_TIME;
    waitForDelegateResponse = false;
  } else {
    resetSolveState();
  }

  stateHasChanged = true;
}

void parseDeviceSettings(JsonChildDocument doc) {
  if (doc["esp_id"] != getEspId()) {
    Logger.println("Wrong deivce settings frame!");
    return;
  }

  if (doc.containsKey("use_inspection")) {
    bool useInspection = doc["use_inspection"];
    state.useInspection = useInspection;
  }

  bool added = doc["added"];
  state.added = added;

  stateHasChanged = true;
}

void parseStartUpdate(JsonChildDocument doc) {
  if (update) {
    ESP.restart();
  }

  if (doc["esp_id"] != getEspId() ||
      doc["version"] == FIRMWARE_VERSION) {
    Logger.println("Cannot start update! (wrong esp id or same firmware version)");
    return;
  }

  sketchSize = sketchSizeRemaining = doc["size"];
  unsigned long maxSketchSize =
      (ESP.getFreeSketchSpace() - 0x1000) & 0xFFFFF000;

  Logger.printf("[Update] Max Sketch Size: %lu | Sketch size: %d\n", maxSketchSize, sketchSizeRemaining);
  
  if (!Update.begin(maxSketchSize)) {
    Update.printError(Serial);
    ESP.restart();
  }

  update = true;
  lcdClear();
  lcdPrintf(0, true, ALIGN_LEFT, "Updating");

  webSocket.sendBIN((uint8_t *)NULL, 0);
}

void parseApiError(JsonChildDocument doc) {
  if (doc["esp_id"] != getEspId()) {
    Logger.println("Wrong api error frame!");
    return;
  }

  String errorMessage = doc["error"];
  bool shouldResetTime = doc["should_reset_time"];
  Logger.printf("Api entry error: %s\n", errorMessage.c_str());

  if(shouldResetTime) resetSolveState();
  if(state.currentScene != SCENE_ERROR) state.sceneBeforeError = state.currentScene;
  state.currentScene = SCENE_ERROR;
  strncpy(state.errorMsg, errorMessage.c_str(), 128);

  waitForSolveResponse = false;
  waitForDelegateResponse = false;
  stateHasChanged = true;
}

void parseTestPacket(JsonChildDocument doc) {
  String type = doc["type"];
  sendTestAck();

  if (type == "Start") {
    state.testMode = true;
    state.lastSolveTime = -1;
  } else if (type == "End") {
    state.testMode = false;
    resetSolveState(true);
  } else if (type == "SolveTime") {
    unsigned long solveTime = doc["data"];
    startSolveSession(solveTime);
  } else if (type == "ButtonPress") {
    JsonArray pinsArr = doc["data"]["pins"].as<JsonArray>();
    std::vector<uint8_t> pins;
    for(JsonVariant v : pinsArr) {
      pins.push_back(v.as<int>());
    }

    int pressTime = doc["data"]["press_time"];
    buttons.testButtonClick(pins, pressTime);
  } else if (type == "ScanCard") {
    unsigned long cardId = doc["data"];
    scanCard(cardId);
  } else if (type == "ResetState") {
    resetSolveState(true);
  } else if (type == "Snapshot") {
    sendSnapshotData();
  }

  stateHasChanged = true;
}

void parseUpdateData(uint8_t *payload, size_t length) {
  if (Update.write(payload, length) != length) {
    Update.printError(Serial);
    Logger.printf("[Update] (lensum) Error! Rebooting...\n");

    delay(250);
    ESP.restart();
  }

  sketchSizeRemaining -= length;
  int percentage = ((sketchSize - sketchSizeRemaining) * 100) / sketchSize;

  lcdPrintf(0, true, ALIGN_LEFT, "Updating (%d%%)", percentage);
  lcdPrintf(1, true, ALIGN_LEFT, "Left: %d", sketchSizeRemaining);

  if (sketchSizeRemaining <= 0) {
    Logger.printf("[Update] Left 0, delay 1s\n");
    delay(1000);

    if (Update.end(true)) {
      Logger.printf("[Update] Success!!! Rebooting...\n");

      delay(250);
      ESP.restart();
    } else {
      Update.printError(Serial);
      Logger.printf("[Update] Error! Rebooting...\n");

      delay(250);
      ESP.restart();
    }
  }

  webSocket.sendBIN((uint8_t *)NULL, 0);
}

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
  if (type == WStype_TEXT) {
    JsonDocument doc;
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      parseCardInfoResponse(doc["card_info_response"]);
    } else if (doc.containsKey("solve_confirm")) {
      parseSolveConfirm(doc["solve_confirm"]);
    } else if (doc.containsKey("delegate_response")) {
      parseDelegateResponse(doc["delegate_response"]);
    } else if (doc.containsKey("device_settings")) {
      parseDeviceSettings(doc["device_settings"]);
    } else if (doc.containsKey("start_update")) {
      parseStartUpdate(doc["start_update"]);
    } else if (doc.containsKey("api_error")) {
      parseApiError(doc["api_error"]);
    } else if (doc.containsKey("test_packet")) {
      parseTestPacket(doc["test_packet"]);
    }
  } else if (type == WStype_BIN) {
    parseUpdateData(payload, length);
  } else if (type == WStype_CONNECTED) {
    Serial.println("Connected to WebSocket server"); // do not send to logger
  } else if (type == WStype_DISCONNECTED) {
    Serial.println("Disconnected from WebSocket server"); // do not send to logger

    // TODO: remove this (if packet queuening is added for backend)
    if(waitForSolveResponse || waitForDelegateResponse) {
      showError("Server not connected!");
      waitForSolveResponse = false;
      waitForDelegateResponse = false;
    }
  }
}

#endif