#ifdef ARDUINO_ARCH_ESP32
  #include <WiFi.h>

  #define ESP_ID() ESP.getEfuseMac()
#else
  #include <ESP8266WiFi.h>
  #include <SoftwareSerial.h>

  #define ESP_ID() ESP.getChipId()
#endif

#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>

#include "utils.hpp"
#include "stackmat.h"
#include "rgb_lcd.h"
#include "ws_logger.h"

#ifdef ARDUINO_ARCH_ESP32
  #define SS_PIN D2
  #define MISO_PIN D3
  #define MOSI_PIN D10
  #define SCK_PIN D8
  #define STACKMAT_TIMER_PIN D7
  #define PLUS2_BUTTON_PIN D1
  #define DNF_BUTTON_PIN D0
#else
  #define SS_PIN 16
  #define SCK_PIN 14
  #define MISO_PIN 12
  #define MOSI_PIN 13
  #define STACKMAT_TIMER_PIN 3
  #define PLUS2_BUTTON_PIN 15
  #define DNF_BUTTON_PIN 0
#endif

// TODO: change ws url
const std::string WS_URL = "ws://192.168.1.38:8080";

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length);
void stackmatLoop();
void lcdLoop();
void buttonsLoop();
void rfidLoop();
void sendSolve();

MFRC522 mfrc522(SS_PIN, UNUSED_PIN); // UNUSED_PIN means that reset is done by software side of that chip
WebSocketsClient webSocket;
Stackmat stackmat;
rgb_lcd lcd;

GlobalState state;
bool stateHasChanged = true;
unsigned long lcdLastDraw = 0;

void setup()
{
  #ifdef ARDUINO_ARCH_ESP32
    Serial.begin(115200);
  #else
    Serial.begin(115200, SERIAL_8N1, SERIAL_TX_ONLY, 1);
  #endif

  EEPROM.begin(512);
  Logger.begin(&Serial, 5000);
  readState(&state);

  #ifdef ARDUINO_ARCH_ESP32
    Serial0.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, STACKMAT_TIMER_PIN, 255, true);
    stackmat.begin(&Serial0);
  #else
    SoftwareSerial stackmatSerial(STACKMAT_TIMER_PIN, -1, true);
    stackmatSerial.begin(STACKMAT_TIMER_BAUD_RATE);
    stackmat.begin(&stackmatSerial);
  #endif

  pinMode(PLUS2_BUTTON_PIN, INPUT_PULLUP);
  pinMode(DNF_BUTTON_PIN, INPUT_PULLUP);

  #ifdef ARDUINO_ARCH_ESP32
    SPI.begin(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  #else
    SPI.pins(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
    SPI.begin();
  #endif
  mfrc522.PCD_Init();

  lcd.begin(16, 2);
  lcd.clear();

  lcd.print("ID: ");
  lcd.setCursor(0, 0);
  lcd.print(getChipID());
  lcd.setCursor(0, 1);
  lcd.print("Connecting...");

  WiFiManager wm;
  // wm.resetSettings();

  String generatedSSID = "StackmatTimer-" + getChipID();
  wm.setConfigPortalTimeout(300);
  bool res = wm.autoConnect(generatedSSID.c_str(), "StackmatTimer");
  if (!res)
  {
    Logger.println("Failed to connect to wifi... Restarting!");
    delay(1500);
    ESP.restart();

    return;
  }

  lcd.clear();
  lcd.setCursor(0, 0);
  lcd.print("WiFi connected!");
  lcd.setCursor(0, 1);

  String ipString = String(WiFi.localIP()[0]) + "." + String(WiFi.localIP()[1]) + "." + String(WiFi.localIP()[2]) + "." + String(WiFi.localIP()[3]);
  lcd.print(ipString);

  std::string host, path;
  int port;

  auto wsRes = parseWsUrl(WS_URL);
  std::tie(host, port, path) = wsRes;

  webSocket.begin(host.c_str(), port, path.c_str());
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);
  Logger.setWsClient(&webSocket);

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
}

void loop() {
  Logger.loop();
  webSocket.loop();
  stackmat.loop();
  lcdLoop();
  buttonsLoop();
  stackmatLoop();
  rfidLoop();
}

void lcdLoop() {
  if (!stateHasChanged || millis() - lcdLastDraw < 50) return;
  stateHasChanged = false;

  lcd.clear();
  lcd.setCursor(0, 0);
  if (state.finishedSolveTime > 0) { // AFTER TIMER IS STOPPED
    uint8_t minutes = state.finishedSolveTime / 60000;
    uint8_t seconds = (state.finishedSolveTime % 60000) / 1000;
    uint16_t ms = state.finishedSolveTime % 1000;
    lcd.printf("%i:%02i.%03i", minutes, seconds, ms);
    if(state.timeOffset == -1) {
      lcd.printf(" DNF");
    } else if (state.timeOffset > 0) {
      lcd.printf(" +%d", state.timeOffset);
    }
    
    if (state.solverCardId == 0) {
      lcd.setCursor(0, 1);
      lcd.printf("Scan card");
    } else if (state.solverCardId > 0 && state.judgeCardId == 0) {
      lcd.setCursor(0, 1);
      lcd.printf("%s", state.solverName.c_str());
    }
  } else if (!stackmat.connected()) { // TIMER DISCONNECTED
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.print("  Disconnected  ");
  } else if (stackmat.state() == StackmatTimerState::ST_Running) { // TIMER IS RUNNING
    lcd.printf("%i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
  } else {
    lcd.printf("    Stackmat    ");
  }

  lcdLastDraw = millis();
}

void buttonsLoop() {
  if (digitalRead(PLUS2_BUTTON_PIN) == LOW) {
    Logger.println("+2 button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(PLUS2_BUTTON_PIN) == LOW) {
      delay(50);
    }

    if (millis() - pressedTime > 5000) {
        Logger.println("Resettings finished solve time!");
        state.timeOffset = 0;
        state.finishedSolveTime = -1;
        state.solverCardId = 0;
        state.judgeCardId = 0;
        stateHasChanged = true;
    } else { 
        if (state.timeOffset != -1) {
            state.timeOffset = state.timeOffset >= 16 ? 0 : state.timeOffset + 2;
            stateHasChanged = true;
        }
    }
  }

  if (digitalRead(DNF_BUTTON_PIN) == LOW) {
    Logger.println("DNF button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(DNF_BUTTON_PIN) == LOW) {
      delay(50);
    }

    if (millis() - pressedTime > 5000) {
      Logger.println("Resetting wifi settings!");
      WiFiManager wm;
      wm.resetSettings();
      delay(1000);
      ESP.restart();
    } else {
      state.timeOffset = state.timeOffset != -1 ? -1 : 0;
      stateHasChanged = true;
    }
  }
}

void rfidLoop() {
  if (state.finishedSolveTime > 0 && millis() - state.lastCardReadTime > 500 && 
      mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial())
  {
    state.lastCardReadTime = millis();

    unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
    Logger.printf("Card ID: %lu\n", cardId);

    JsonDocument doc;
    doc["card_info_request"]["card_id"] = cardId;
    doc["card_info_request"]["esp_id"] = ESP_ID();

    String json;
    serializeJson(doc, json);
    webSocket.sendTXT(json);
  }
}

void stackmatLoop()
{
  if (stackmat.state() != state.lastTiemrState && stackmat.state() != ST_Unknown && state.lastTiemrState != ST_Unknown)
  {
    Logger.printf("State changed from %c to %c\n", state.lastTiemrState, stackmat.state());
    switch (stackmat.state())
    {
      case ST_Stopped:
        if (state.finishedSolveTime > 0) break;

        Logger.printf("FINISH! Final time is %i:%02i.%03i!\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        state.finishedSolveTime = stackmat.time();

        saveState(state);
        // writeEEPROMInt(4, state.finishedSolveTime);
        // EEPROM.commit();
        break;

      case ST_Reset:
        Logger.println("Timer reset!");
        break;

      case ST_Running:
        if (state.finishedSolveTime > 0) break;
        state.solveSessionId++;
        state.finishedSolveTime = -1;

        Logger.println("Solve started!");
        Logger.printf("Solve session ID: %i\n", state.solveSessionId);
        // writeEEPROMInt(0, state.solveSessionId);
        break;

      default:
        break;
    }

    stateHasChanged = true;
  }

  if (stackmat.state() == StackmatTimerState::ST_Running) {
    stateHasChanged = true;
  } else if (stackmat.connected() != state.stackmatConnected) {
    state.stackmatConnected = stackmat.connected();
    stateHasChanged = true;
  }

  state.lastTiemrState = stackmat.state();
}

void sendSolve() {
  if (state.finishedSolveTime == -1) return;

  struct tm timeinfo;
  if (!getLocalTime(&timeinfo))
  {
    Logger.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);
  
  JsonDocument doc;
  doc["solve"]["solve_time"] = state.finishedSolveTime;
  doc["solve"]["card_id"] = state.solverCardId;
  doc["solve"]["esp_id"] = ESP_ID();
  doc["solve"]["timestamp"] = epoch;
  doc["solve"]["session_id"] = state.solveSessionId;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length)
{
  if (type == WStype_TEXT) {
    JsonDocument doc;
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      String name = doc["card_info_response"]["name"];
      unsigned long cardId = doc["card_info_response"]["card_id"];

      // here just check card type (if its judge's or player's)
      if (state.solverCardId == 0) {
        state.solverName = name;
        state.solverCardId = cardId;
      } else if(state.solverCardId > 0 && state.judgeCardId == 0) { 
        //judge is always scanning his card last
        state.judgeCardId = cardId;

        sendSolve();
      }

      stateHasChanged = true;
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
      stateHasChanged = true;
    }

    // Logger.printf("Received message: %s\n", doc["espId"].as<const char *>());
  }
  else if (type == WStype_CONNECTED) {
    JsonDocument doc;
    doc["connect"]["esp_id"] = ESP_ID();

    String json;
    serializeJson(doc, json);
    webSocket.sendTXT(json);

    Logger.println("Connected to WebSocket server");
  }
  else if (type == WStype_DISCONNECTED) {
    Logger.println("Disconnected from WebSocket server");
  }
}
