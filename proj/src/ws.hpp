#ifndef __WS_HPP__
#define __WS_HPP__

#if defined(ESP32)
#include <Update.h>
#include <WiFi.h>

#define CHIP "esp32c3"
#elif defined(ESP8266)
#include <ESP8266WiFi.h>
#include <Updater.h>

#define CHIP "esp8266"
#endif

#include <WiFiManager.h>

#include "globals.hpp"
#include "lcd.hpp"
#include "utils.hpp"

#define WIFI_SSID_PREFIX "FkmTimer-"
#define WIFI_PASSWORD "FkmTimer"

inline void webSocketEvent(WStype_t type, uint8_t *payload, size_t length);

String wsURL = "";

// updater stuff (OTA)
int sketchSize = 0;
int sketchSizeRemaining = 0;
bool update = false;

// inits wifimanager and websockets client
inline void netInit() {
  WiFiManager wm;

  String generatedSSID = WIFI_SSID_PREFIX + getChipHex();
  wm.setConfigPortalTimeout(300);
  bool res = wm.autoConnect(generatedSSID.c_str(), WIFI_PASSWORD);
  if (!res) {
    Logger.println("Failed to connect to wifi... Restarting!");
    delay(1500);
    ESP.restart();
  }

  lcdPrintf(0, true, ALIGN_CENTER, "FKM");
  lcdPrintf(1, true, ALIGN_CENTER, "Looking for MDNS");

  while (true) {
    wsURL = getWsUrl();
    if (wsURL.length() > 0)
      break;
    delay(1000);
  }

  std::string host, path;
  int port;

  auto wsRes = parseWsUrl(wsURL.c_str());
  std::tie(host, port, path) = wsRes;

  char finalPath[128];
  snprintf(finalPath, 128, "%s?id=%lu&ver=%s&chip=%s&bt=%s", path.c_str(),
           ESP_ID(), FIRMWARE_VERSION, CHIP, BUILD_TIME);

  webSocket.begin(host.c_str(), port, finalPath);
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);
  Logger.setWsClient(&webSocket);

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
}

inline void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
  if (type == WStype_TEXT) {
    JsonDocument doc;
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      String display = doc["card_info_response"]["display"];
      unsigned long cardId = doc["card_info_response"]["card_id"];
      String countryIso2 = doc["card_info_response"]["country_iso2"];
      countryIso2.toLowerCase();

      if (state.competitorCardId > 0 && state.judgeCardId > 0 && state.competitorCardId == cardId && millis() - state.lastTimeSent > 1500) {
        state.lastTimeSent = millis();
        sendSolve(false);
      } else if (state.competitorCardId > 0 && state.competitorCardId != cardId && state.finishedSolveTime > 0 && state.timeConfirmed) {
        state.judgeCardId = cardId;
      } else if (state.competitorCardId == 0) {
        state.competitorDisplay = display;
        state.competitorCardId = cardId;
        primaryLangauge = countryIso2 != "pl";

        if (state.lastFinishedSolveTime != stackmat.time()) {
          state.solveSessionId++;
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

      state.finishedSolveTime = -1;
      state.timeOffset = 0;
      state.competitorCardId = 0;
      state.judgeCardId = 0;
      state.competitorDisplay = "";
      state.timeStarted = false;
      state.timeConfirmed = false;
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
  } else if (type == WStype_DISCONNECTED) {
    Logger.println("Disconnected from WebSocket server");
  }
}

#endif
