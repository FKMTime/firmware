#ifndef __WS_HPP__
#define __WS_HPP__

#if defined(ESP32)
  #include <WiFi.h>
  #include <Update.h>

  #define CHIP "esp32c3"
#elif defined(ESP8266)
  #include <ESP8266WiFi.h>
  #include <Updater.h>

  #define CHIP "esp8266"
#endif

#include <WiFiManager.h>

#include "globals.hpp"
#include "utils.hpp"
#include "lcd.hpp"

inline void webSocketEvent(WStype_t type, uint8_t *payload, size_t length);

String wsURL = "";

// updater stuff (OTA)
int sketchSize = 0;
bool update = false;

// inits wifimanager and websockets client
inline void netInit() {
    WiFiManager wm;

    String generatedSSID = "StackmatTimer-" + getChipHex();
    wm.setConfigPortalTimeout(300);
    bool res = wm.autoConnect(generatedSSID.c_str(), "StackmatTimer");
    if (!res)
    {
        Logger.println("Failed to connect to wifi... Restarting!");
        delay(1500);
        ESP.restart();
    }

    lcd.clear();
    lcd.setCursor(0, 0);
    lcd.printf("%s", centerString("Stackmat", 16).c_str());
    lcd.setCursor(0, 1);
    lcd.printf("%s", centerString("Looking for MDNS", 16).c_str());

    while(true) {
        wsURL = getWsUrl();
        if (wsURL.length() > 0) break;
        delay(1000);
    }

    std::string host, path;
    int port;

    auto wsRes = parseWsUrl(wsURL.c_str());
    std::tie(host, port, path) = wsRes;

    char finalPath[128];
    snprintf(finalPath, 128, "%s?id=%lu&ver=%s&chip=%s", path.c_str(), ESP_ID(), FIRMWARE_VERSION, CHIP);

    webSocket.begin(host.c_str(), port, finalPath);
    webSocket.onEvent(webSocketEvent);
    webSocket.setReconnectInterval(5000);
    Logger.setWsClient(&webSocket);

    configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
}

inline void webSocketEvent(WStype_t type, uint8_t *payload, size_t length)
{
  if (type == WStype_TEXT) {
    JsonDocument doc;
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      String name = doc["card_info_response"]["name"];
      unsigned long cardId = doc["card_info_response"]["card_id"];
      bool isJudge = doc["card_info_response"]["is_judge"];

      if (isJudge && state.solverCardId > 0 && state.finishedSolveTime > 0 && state.timeConfirmed && millis() - state.lastTimeSent > 1500) {
        state.judgeCardId = cardId;
        state.lastTimeSent = millis();
        sendSolve(false);
      } else if(!isJudge && state.solverCardId == 0) {
        state.solverName = name;
        state.solverCardId = cardId;
      }

      lcdChange();
    } else if (doc.containsKey("solve_confirm")) {
      if (doc["solve_confirm"]["card_id"] != state.solverCardId || 
          doc["solve_confirm"]["esp_id"] != ESP_ID() || 
          doc["solve_confirm"]["session_id"] != state.solveSessionId) {
        Logger.println("Wrong solve confirm frame!");
        return;
      }

      state.finishedSolveTime = -1;
      state.solverCardId = 0;
      state.judgeCardId = 0;
      state.solverName = "";
      state.timeStarted = false;
      saveState();
      lcdChange();
    } else if (doc.containsKey("start_update")) {
      if (update) {
        ESP.restart();
      }

      if (doc["start_update"]["esp_id"] != ESP_ID() || doc["start_update"]["version"] == FIRMWARE_VERSION) {
        Logger.println("Cannot start update!");
        return;
      }

      sketchSize = doc["start_update"]["size"];
      unsigned long maxSketchSize = (ESP.getFreeSketchSpace() - 0x1000) & 0xFFFFF000;

      Logger.printf("[Update] Max Sketch Size: %lu | Sketch size: %d\n", maxSketchSize, sketchSize);
      if (!Update.begin(maxSketchSize)) {
        Update.printError(Serial);
        ESP.restart();
      }

      update = true;
      lcd.clear();
      lcd.setCursor(0,0);
      lcd.printf("Updating...");
    }
  }
  else if (type == WStype_BIN) {
    if (Update.write(payload, length) != length) {
      Update.printError(Serial);
      ESP.restart();
    }

    yield();
    sketchSize -= length;
    lcd.setCursor(0,1);
    lcd.printf("                "); // clear second line
    lcd.setCursor(0,1);
    lcd.printf("Left: %d", sketchSize);

    if (sketchSize <= 0) {
      if (Update.end(true)) {
        Logger.printf("[Update] Success!!! Rebooting...\n");
        delay(5);
        yield();
        ESP.restart();
      } else {
        Update.printError(Serial);
        ESP.restart();
      }
    }

    webSocket.sendBIN((uint8_t *)NULL, 0);
  }
  else if (type == WStype_CONNECTED) {
    Logger.println("Connected to WebSocket server");
  }
  else if (type == WStype_DISCONNECTED) {
    Logger.println("Disconnected from WebSocket server");
  }
}

#endif