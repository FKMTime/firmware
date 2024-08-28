#include "websocket.hpp"

#include "../utils.hpp"
#include "Update.h"
#include "buttons.hpp"
#include "globals.hpp"
#include "lcd.hpp"
#include "state.hpp"
#include "utils.hpp"
#include "version.h"
#include "ws_logger.h"

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
           wsInfo.path, getEspId(), FIRMWARE_VERSION, CHIP, BUILD_TIME,
           FIRMWARE_TYPE);

  if (wsInfo.ssl) {
    webSocket.beginSSL(wsInfo.host, wsInfo.port, finalPath);
  } else {
    webSocket.begin(wsInfo.host, wsInfo.port, finalPath);
  }

  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(1500);
  Logger.setWsClient(&webSocket);
}

void parseCardInfoResponse(JsonObject doc) {
  String display = doc["display"];
  unsigned long cardId = doc["card_id"];
  String countryIso2 = doc["country_iso2"];
  bool canCompete = doc["can_compete"];
  countryIso2.toLowerCase();

  if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR ||
      state.currentScene == SCENE_WAITING_FOR_COMPETITOR_WITH_TIME) {
    if (!webSocket.isConnected() || (!stackmat.connected() && !state.testMode))
      return;

    if (state.competitorCardId == 0 && canCompete) {
      strncpy(state.competitorDisplay, display.c_str(), 128);
      state.competitorCardId = cardId;
      primaryLangauge = countryIso2 != "pl";
      state.currentScene = SCENE_COMPETITOR_INFO;

      int time = state.testMode ? testModeStackmatTime : stackmat.time();
      if (state.solveTime != time) {
        if (state.useInspection)
          stopInspection();

        if (state.lastTimerState == ST_Stopped || state.testMode) {
          endSolveSession(time);
        } else {
          state.currentScene = SCENE_TIMER_TIME;
        }
      }
    }
  } else if (state.currentScene == SCENE_FINISHED_TIME) {
    if (state.competitorCardId != cardId && state.timeConfirmed) {
      state.judgeCardId = cardId;
    } else if (state.judgeCardId > 0 && state.competitorCardId == cardId) {
      sendSolve(false);
    }
  }

  stateHasChanged = true;
}

void parseSolveConfirm(JsonObject doc) {
  if (doc["competitor_id"] != state.competitorCardId ||
      doc["esp_id"] != getEspId() ||
      doc["session_id"] != state.solveSessionId) {
    Logger.println("Wrong solve confirm frame!");
    return;
  }

  resetSolveState();
}

void parseDelegateResponse(JsonObject doc) {
  if (doc["esp_id"] != getEspId()) {
    Logger.println("Wrong solve confirm frame!");
    return;
  }

  if (doc.containsKey("solve_time")) {
    unsigned long solveTime = doc["solve_time"];
    state.solveTime = solveTime;
  }

  if (doc.containsKey("penalty")) {
    int penalty = doc["penalty"];
    state.penalty = penalty;
  }

  bool shouldScanCards = doc["should_scan_cards"];
  state.timeConfirmed = true;

  if (shouldScanCards) {
    state.currentScene = SCENE_FINISHED_TIME;
    waitForDelegateResponse = false;
  } else {
    resetSolveState();
  }

  stateHasChanged = true;
}

void parseDeviceSettings(JsonObject doc) {
  if (doc["esp_id"] != getEspId()) {
    Logger.println("Wrong deivce settings frame!");
    return;
  }

  if (doc.containsKey("use_inspection")) {
    bool useInspection = doc["use_inspection"];
    state.useInspection = useInspection;
  }

  if (doc.containsKey("secondary_text")) {
    String secondaryText = doc["secondary_text"];
    strncpy(state.secondaryText, secondaryText.c_str(), 32);
  }

  bool added = doc["added"];
  state.added = added;

  stateHasChanged = true;
}

void parseEpochTime(JsonObject doc) {
  epochBase = doc["current_epoch"];
  epochBase -= millis() / 1000;
}

void parseStartUpdate(JsonObject doc) {
  if (update) {
    ESP.restart();
  }

  if (doc["esp_id"] != getEspId() || doc["version"] == FIRMWARE_VERSION) {
    Logger.println(
        "Cannot start update! (wrong esp id or same firmware version)");
    return;
  }

  sketchSize = sketchSizeRemaining = doc["size"];
  unsigned long maxSketchSize =
      (ESP.getFreeSketchSpace() - 0x1000) & 0xFFFFF000;

  Logger.printf("[Update] Max Sketch Size: %lu | Sketch size: %d\n",
                maxSketchSize, sketchSizeRemaining);

  if (!Update.begin(maxSketchSize)) {
    Update.printError(Serial);
    ESP.restart();
  }

  update = true;
  lcdClear();
  lcdPrintf(0, true, ALIGN_LEFT, "Updating");

  webSocket.sendBIN((uint8_t *)NULL, 0);
}

void parseApiError(JsonObject doc) {
  if (doc["esp_id"] != getEspId()) {
    Logger.println("Wrong api error frame!");
    return;
  }

  String errorMessage = doc["error"];
  bool shouldResetTime = doc["should_reset_time"];
  Logger.printf("Api entry error: %s\n", errorMessage.c_str());

  if (shouldResetTime)
    resetSolveState();
  if (state.currentScene != SCENE_ERROR)
    state.sceneBeforeError = state.currentScene;
  state.currentScene = SCENE_ERROR;
  strncpy(state.errorMsg, errorMessage.c_str(), 128);

  waitForSolveResponse = false;
  waitForDelegateResponse = false;
  stateHasChanged = true;
}

void parseTestPacket(JsonObject doc) {
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
    testModeStackmatTime = solveTime;

    if (state.competitorCardId > 0) {
      endSolveSession(solveTime);
    } else {
      state.currentScene = SCENE_WAITING_FOR_COMPETITOR_WITH_TIME;
    }
  } else if (type == "ButtonPress") {
    JsonArray pinsArr = doc["data"]["pins"].as<JsonArray>();
    std::vector<uint8_t> pins;
    for (JsonVariant v : pinsArr) {
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
    // TODO: idk if not too messy
    Logger.printf("Received websocket message: %s", payload);

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
    } else if (doc.containsKey("epoch_time")) {
      parseEpochTime(doc["epoch_time"]);
    }
  } else if (type == WStype_BIN) {
    parseUpdateData(payload, length);
  } else if (type == WStype_CONNECTED) {
    Serial.println("Connected to WebSocket server"); // do not send to logger
  } else if (type == WStype_DISCONNECTED) {
    Serial.println(
        "Disconnected from WebSocket server"); // do not send to logger

    // TODO: remove this (if packet queuening is added for backend)
    if (waitForSolveResponse || waitForDelegateResponse) {
      showError("Server not connected!");
      waitForSolveResponse = false;
      waitForDelegateResponse = false;
    }
  }
}
