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

  char finalPath[128];
  snprintf(finalPath, 128, "%s?id=%lu&ver=%s&chip=%s&bt=%s", 
            wsInfo.path, (unsigned long)ESP.getEfuseMac(), FIRMWARE_VERSION, CHIP, BUILD_TIME);

  webSocket.begin(wsInfo.host, wsInfo.port, finalPath);
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);
  Logger.setWsClient(&webSocket);
}

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
  /*
  if (type == WStype_TEXT) {
    JsonDocument doc;
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      state.lastCardReadTime = millis();

      String display = doc["card_info_response"]["display"];
      unsigned long cardId = doc["card_info_response"]["card_id"];
      String countryIso2 = doc["card_info_response"]["country_iso2"];
      bool canCompete = doc["card_info_response"]["can_compete"];
      countryIso2.toLowerCase();

      if (state.competitorCardId > 0 && state.judgeCardId > 0 && state.competitorCardId == cardId && millis() - state.lastTimeSent > 1500) {
        state.lastTimeSent = millis();
        sendSolve(false);
      } else if (state.competitorCardId > 0 && state.competitorCardId != cardId && state.finishedSolveTime > 0 && state.timeConfirmed) {
        state.judgeCardId = cardId;
      } else if (state.competitorCardId == 0 && canCompete) {
        state.competitorDisplay = display;
        state.competitorCardId = cardId;
        primaryLangauge = countryIso2 != "pl";

        if (state.lastFinishedSolveTime != stackmat.time()) {
          strcpy(state.solveSessionId, generateUUID());
          state.timeStarted = true;
          state.timeConfirmed = false;
          state.timeOffset = 0;
          state.finishedSolveTime = stackmat.time();
          state.lastFinishedSolveTime = state.finishedSolveTime;
        }
      }

      lcdChange();
    } else if (doc.containsKey("solve_confirm")) {
      if (doc["solve_confirm"]["competitor_id"] != state.competitorCardId ||
          doc["solve_confirm"]["esp_id"] != ESP_ID() ||
          doc["solve_confirm"]["session_id"] != state.solveSessionId) {
        Logger.println("Wrong solve confirm frame!");
        return;
      }

      state.lastCardReadTime = millis();
      state.finishedSolveTime = -1;
      state.timeOffset = 0;
      state.competitorCardId = 0;
      state.judgeCardId = 0;
      state.competitorDisplay = "";
      state.timeStarted = false;
      state.timeConfirmed = false;
      state.waitingForSolveResponse = false;
      saveState();
      lcdChange();
    } else if (doc.containsKey("start_update")) {
      if (update) {
        ESP.restart();
      }

      if (doc["start_update"]["esp_id"] != ESP_ID() ||
          doc["start_update"]["version"] == FIRMWARE_VERSION) {
        Logger.println("Cannot start update!");
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
      if (doc["api_error"]["esp_id"] != ESP_ID()) {
        Logger.println("Wrong api error frame!");
        return;
      }

      String errorMessage = doc["api_error"]["error"];
      bool shouldResetTime = doc["api_error"]["should_reset_time"];
      Logger.printf("Api entry error: %s\n", errorMessage.c_str());

      if (shouldResetTime) {
        state.finishedSolveTime = -1;
        state.competitorCardId = 0;
        state.judgeCardId = 0;
        state.competitorDisplay = "";
        state.timeStarted = false;
        state.waitingForSolveResponse = false;
        saveState();
      }

      lcdPrintf(0, true, ALIGN_CENTER, TR_ERROR_HEADER);
      lcdPrintf(1, true, ALIGN_CENTER, errorMessage.c_str());

      state.errored = true;
    }
  } else if (type == WStype_BIN) {
    if (Update.write(payload, length) != length) {
      Update.printError(Serial);
      Logger.printf("[Update] (lensum) Error! Rebooting...\n");
      webSocket.loop();

      delay(250);
      ESP.restart();
    }

    sketchSizeRemaining -= length;
    int percentage = ((sketchSize - sketchSizeRemaining) * 100) / sketchSize;

    lcdPrintf(0, true, ALIGN_LEFT, "Updating (%d%%)", percentage);
    lcdPrintf(1, true, ALIGN_LEFT, "Left: %d", sketchSizeRemaining);

    if (sketchSizeRemaining <= 0) {
      Logger.printf("[Update] Left 0, delay 1s\n");
      webSocket.loop();
      delay(1000);

      if (Update.end(true)) {
        Logger.printf("[Update] Success!!! Rebooting...\n");
        webSocket.loop();

        delay(250);
        ESP.restart();
      } else {
        Update.printError(Serial);
        Logger.printf("[Update] Error! Rebooting...\n");
        webSocket.loop();

        delay(250);
        ESP.restart();
      }
    }

    webSocket.sendBIN((uint8_t *)NULL, 0);
  } else if (type == WStype_CONNECTED) {
    Logger.println("Connected to WebSocket server");

    if(state.waitingForSolveResponse) {
      sendSolve(false); // re-send time when waiting for solve response
    }
  } else if (type == WStype_DISCONNECTED) {
    Logger.println("Disconnected from WebSocket server");
  }
  */
}

#endif