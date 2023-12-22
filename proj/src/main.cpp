#include <Arduino.h>
#include <ESP8266WiFi.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>
#include <SoftwareSerial.h>

#include "utils.hpp"
#include "stackmat.h"
#include "rgb_lcd.h"

#define RST_PIN A0
#define SS_PIN 16
#define SCK_PIN 14
#define MISO_PIN 12
#define MOSI_PIN 13
#define STACKMAT_TIMER_PIN 2
#define OK_BUTTON_PIN 3
#define PLUS2_BUTTON_PIN 15
#define DNF_BUTTON_PIN 0

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length);
void stackmatLoop();
void lcdLoop();
void buttonsLoop();
void sendSolve();

SoftwareSerial stackmatSerial(STACKMAT_TIMER_PIN, -1, true);
MFRC522 mfrc522(SS_PIN, UNUSED_PIN); // UNUSED_PIN means that reset is done by software side of that chip
WebSocketsClient webSocket;
Stackmat stackmat;
rgb_lcd lcd;

GlobalState state;
bool stateHasChanged = false;
unsigned long lcdLastDraw = 0;

void setup()
{
  Serial.begin(115200, SERIAL_8N1, SERIAL_TX_ONLY, 1);

  // LOAD SOLVE SESSION ID, FINISHED SOLVE TIME FROM EEPROM
  state.solveSessionId = 0;
  state.finishedSolveTime = -1;
  state.timeOffset = 0;
  state.lastCardReadTime = 0;
  state.solverCardId = 0;
  state.solverName = "";
  stateHasChanged = true;

  stackmatSerial.begin(1200);
  stackmat.begin(&stackmatSerial);

  pinMode(PLUS2_BUTTON_PIN, INPUT_PULLUP);
  pinMode(DNF_BUTTON_PIN, INPUT_PULLUP);
  pinMode(OK_BUTTON_PIN, INPUT_PULLUP);

  SPI.pins(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  SPI.begin();
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
    Serial.println("Failed to connect to wifi... Restarting!");
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

  webSocket.begin("192.168.1.38", 8080, "/");
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
}

void loop() {
  webSocket.loop();
  stackmat.loop();
  lcdLoop();
  buttonsLoop();

  if (state.finishedSolveTime > 0 && millis() - state.lastCardReadTime > 1500 && 
      mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial())
  {
    state.lastCardReadTime = millis();

    unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
    Serial.print("Card ID: ");
    Serial.println(cardId);

    DynamicJsonDocument doc(256);
    doc["card_info_request"]["card_id"] = cardId;
    doc["card_info_request"]["esp_id"] = ESP.getChipId();

    // struct tm timeinfo;
    // if (!getLocalTime(&timeinfo))
    // {
    //   Serial.println("Failed to obtain time");
    // }
    // time_t epoch;
    // time(&epoch);

    
    // doc["solve"]["solve_time"] = finishedSolveTime;
    // doc["solve"]["card_id"] = cardId;
    // doc["solve"]["esp_id"] = ESP.getChipId();
    // doc["solve"]["timestamp"] = epoch;
    // doc["solve"]["session_id"] = solveSessionId;

    String json;
    serializeJson(doc, json);
    webSocket.sendTXT(json);
  }

  stackmatLoop();
}

void lcdLoop() {
  if (!stateHasChanged || millis() - lcdLastDraw < 50) return;

  lcd.clear();
  lcd.setCursor(0, 0);
  if (state.finishedSolveTime > 0) { // AFTER TIMER IS STOPPED (AFTER TIMER WAS RUNNING)
    lcd.printf("%i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
    if(state.timeOffset == -1) {
      lcd.printf(" DNF");
    } else if (state.timeOffset > 0) {
      lcd.printf(" +%d", state.timeOffset);
    }
    
    if (state.solverName.length() > 0) {
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
  stateHasChanged = false;
}

void buttonsLoop() {
  if (digitalRead(OK_BUTTON_PIN) == LOW) {
    Serial.println("OK button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(OK_BUTTON_PIN) == LOW) {
      delay(50);
    }

    if (millis() - pressedTime > 5000) {
      // THIS SHOULD BE ON +2 BTN
      Serial.println("Resettings finished solve time!");
      state.finishedSolveTime = -1;
      stateHasChanged = true;
    } else {
      sendSolve();
      stateHasChanged = true;
    }
  }

  if (digitalRead(PLUS2_BUTTON_PIN) == HIGH) {
    Serial.println("+2 button pressed!");
    //unsigned long pressedTime = millis();
    while (digitalRead(PLUS2_BUTTON_PIN) == HIGH) {
      delay(50);
    }

    if (state.timeOffset != -1) {
      state.timeOffset = state.timeOffset >= 16 ? 0 : state.timeOffset + 2;
      stateHasChanged = true;
    }
  }

  if (digitalRead(DNF_BUTTON_PIN) == LOW) {
    Serial.println("DNF button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(DNF_BUTTON_PIN) == LOW) {
      delay(50);
    }

    if (millis() - pressedTime > 10000) {
      Serial.println("Resetting wifi settings!");
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

void stackmatLoop()
{
  if (stackmat.state() != state.lastTiemrState && stackmat.state() != ST_Unknown && state.lastTiemrState != ST_Unknown)
  {
    Serial.printf("State changed from %c to %c\n", state.lastTiemrState, stackmat.state());
    switch (stackmat.state())
    {
      case ST_Stopped:
        Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        state.finishedSolveTime = stackmat.time();

        writeEEPROMInt(4, state.finishedSolveTime);
        EEPROM.commit();
        break;

      case ST_Reset:
        Serial.println("Timer reset!");
        break;

      case ST_Running:
        state.solveSessionId++;
        state.finishedSolveTime = -1;

        Serial.println("Solve started!");
        Serial.printf("Solve session ID: %i\n", state.solveSessionId);
        writeEEPROMInt(0, state.solveSessionId);
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
    Serial.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);
  
  DynamicJsonDocument doc(256);
  doc["solve"]["solve_time"] = state.finishedSolveTime;
  doc["solve"]["card_id"] = state.solverCardId;
  doc["solve"]["esp_id"] = ESP.getChipId();
  doc["solve"]["timestamp"] = epoch;
  doc["solve"]["session_id"] = state.solveSessionId;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length)
{
  if (type == WStype_TEXT) {
    DynamicJsonDocument doc(2048);
    deserializeJson(doc, payload);

    if (doc.containsKey("card_info_response")) {
      String name = doc["card_info_response"]["name"];
      unsigned long cardId = doc["card_info_response"]["card_id"];

      state.solverName = name;
      state.solverCardId = cardId;
      stateHasChanged = true;
    } else if (doc.containsKey("solve_confirm")) {
      if (doc["solve_confirm"]["card_id"] != state.solverCardId || 
          doc["solve_confirm"]["esp_id"] != ESP.getChipId() || 
          doc["solve_confirm"]["session_id"] != state.solveSessionId) {
        Serial.println("Wrong solve confirm frame!");
        return;
      }

      state.finishedSolveTime = -1;
      state.solverCardId = 0;
      state.solverName = "";
      stateHasChanged = true;
    }

    // Serial.printf("Received message: %s\n", doc["espId"].as<const char *>());
  }
  else if (type == WStype_CONNECTED) {
    DynamicJsonDocument doc(256);
    doc["connect"]["esp_id"] = ESP.getChipId();

    String json;
    serializeJson(doc, json);
    webSocket.sendTXT(json);

    Serial.println("Connected to WebSocket server");
  }
  else if (type == WStype_DISCONNECTED) {
    Serial.println("Disconnected from WebSocket server");
  }
}