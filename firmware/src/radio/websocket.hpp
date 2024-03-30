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

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
  if (type == WStype_TEXT) {
    JsonDocument doc;
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      String display = doc["card_info_response"]["display"];
      unsigned long cardId = doc["card_info_response"]["card_id"];
      String countryIso2 = doc["card_info_response"]["country_iso2"];
      bool canCompete = doc["card_info_response"]["can_compete"];
      countryIso2.toLowerCase();

      if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR) {
        if(!webSocket.isConnected() || !stackmat.connected()) return;

        if (state.competitorCardId == 0 && canCompete) {
          strncpy(state.competitorDisplay, display.c_str(), 128);
          state.competitorCardId = cardId;
          primaryLangauge = countryIso2 != "pl";
          state.currentScene = SCENE_COMPETITOR_INFO;

          if (state.solveTime != stackmat.time()) {
            startSolveSession(stackmat.time());
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
    } else if (doc.containsKey("solve_confirm")) {
      if (doc["solve_confirm"]["competitor_id"] != state.competitorCardId ||
          doc["solve_confirm"]["esp_id"] != getEspId() ||
          doc["solve_confirm"]["session_id"] != state.solveSessionId) {
        Logger.println("Wrong solve confirm frame!");
        return;
      }

      resetSolveState();
    } else if (doc.containsKey("device_settings")) {
      if (doc["device_settings"]["esp_id"] != getEspId()) {
        Logger.println("Wrong deivce settings frame!");
        return;
      }

      bool useInspection = doc["device_settings"]["use_inspection"];
      bool added = doc["device_settings"]["added"];
      state.useInspection = useInspection;
      state.added = added;
      stateHasChanged = true;
    } else if (doc.containsKey("start_update")) {
      if (update) {
        ESP.restart();
      }

      if (doc["start_update"]["esp_id"] != getEspId() ||
          doc["start_update"]["version"] == FIRMWARE_VERSION) {
        Logger.println("Cannot start update! (wrong esp id or same firmware version)");
        return;
      }

      sketchSize = sketchSizeRemaining = doc["start_update"]["size"];
      unsigned long maxSketchSize =
          (ESP.getFreeSketchSpace() - 0x1000) & 0xFFFFF000;

      Logger.printf("[Update] Max Sketch Size: %lu | Sketch size: %d\n",
                    maxSketchSize, sketchSizeRemaining);
      if (!Update.begin(maxSketchSize)) {
        Update.printError(Serial);
        ESP.restart();
      }

      update = true;
      lcdPrintf(0, true, ALIGN_LEFT, "Updating");
      lcdClearLine(1);

      webSocket.sendBIN((uint8_t *)NULL, 0);
    } else if (doc.containsKey("api_error")) {
      if (doc["api_error"]["esp_id"] != getEspId()) {
        Logger.println("Wrong api error frame!");
        return;
      }

      String errorMessage = doc["api_error"]["error"];
      bool shouldResetTime = doc["api_error"]["should_reset_time"];
      Logger.printf("Api entry error: %s\n", errorMessage.c_str());

      if(shouldResetTime) resetSolveState();
      state.sceneBeforeError = state.currentScene;
      state.currentScene = SCENE_ERROR;
      strncpy(state.errorMsg, errorMessage.c_str(), 128);

      waitForSolveResponse = false;
      stateHasChanged = true;
    }
  } else if (type == WStype_BIN) {
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
  } else if (type == WStype_CONNECTED) {
    Serial.println("Connected to WebSocket server"); // do not send to logger
  } else if (type == WStype_DISCONNECTED) {
    Serial.println("Disconnected from WebSocket server"); // do not send to logger

    if(waitForSolveResponse) {
      showError("Server not connected!");
      waitForSolveResponse = false;
    }
  }
}

#endif