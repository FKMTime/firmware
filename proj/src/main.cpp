#if defined(ESP32)
  #define ESP_ID() (unsigned long)ESP.getEfuseMac()

  #define CS_PIN D2
  #define MISO_PIN D3
  #define MOSI_PIN D10
  #define SCK_PIN D8
  #define STACKMAT_TIMER_PIN D7
  #define PENALTY_BUTTON_PIN D1
  #define SUBMIT_BUTTON_PIN D0
#elif defined(ESP8266)
  #define ESP_ID() (unsigned long)ESP.getChipId()

  #define CS_PIN 16
  #define SCK_PIN 14
  #define MISO_PIN 12
  #define MOSI_PIN 13
  #define STACKMAT_TIMER_PIN 3
  #define PENALTY_BUTTON_PIN 0
  #define SUBMIT_BUTTON_PIN 2
  #define DELEGATE_BUTTON_PIN 15
#endif

#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <CstmSoftwareSerial.h>

#include "version.h"
#include "utils.hpp"
#include "ws_logger.h"
#include "ws.hpp"
#include "lcd.hpp"
#include "buttons.hpp"
#include "globals.hpp"

void stackmatLoop();
void rfidLoop();

SoftwareSerial stackmatSerial(STACKMAT_TIMER_PIN, -1, true);
MFRC522 mfrc522(CS_PIN, UNUSED_PIN);

bool lastWebsocketState = false;

char hostString[16] = {0};
void setup()
{
  #if defined(ESP32)
    Serial.begin(115200);
  #elif defined(ESP8266)
    Serial.begin(115200, SERIAL_8N1, SERIAL_TX_ONLY, 1); //IT WONT WORK, because im setting that pin as input (TODO: debug mode)
  #endif

  Logger.begin(&Serial, 5000);
  Logger.printf("Current firmware version: %s\n", FIRMWARE_VERSION);

  EEPROM.begin(128);
  readState();

  pinMode(PENALTY_BUTTON_PIN, INPUT_PULLUP);
  pinMode(SUBMIT_BUTTON_PIN, INPUT_PULLUP);
  pinMode(DELEGATE_BUTTON_PIN, INPUT_PULLUP);

  stackmatSerial.begin(STACKMAT_TIMER_BAUD_RATE);
  // stackmatSerial.setResend(STACKMAT_DISPLAY_PIN);
  stackmat.begin(&stackmatSerial);

  #if defined(ESP32)
    SPI.begin(SCK_PIN, MISO_PIN, MOSI_PIN, CS_PIN);
  #elif defined(ESP8266)
    SPI.pins(SCK_PIN, MISO_PIN, MOSI_PIN, CS_PIN);
    SPI.begin();
  #endif
  mfrc522.PCD_Init();

  lcdInit();
  lcdClear();

  lcdPrintf(0, true, ALIGN_LEFT, "ID: %s", getChipHex().c_str());
  lcdPrintf(1, true, ALIGN_LEFT, "VER: %s", FIRMWARE_VERSION);

  netInit();
}

void loop() {
  if (sleepMode) {
    delay(100);
    buttonsLoop();
    return;
  }

  webSocket.loop();
  if (!update) {
    Logger.loop();
    stackmat.loop();
    stackmatLoop();

    // functions that are useless while timer is running:
    if(stackmat.state() != ST_Running) {
      buttonsLoop();
      rfidLoop();
    }


    // it will only occur when time was sent but not processed by server (because of error)
    if (state.finishedSolveTime > 0 && state.judgeCardId > 0 && millis() - state.lastTimeSent > 1500) {
      state.judgeCardId = 0;
    }
    
    lcdLoop();
  }

  if (lastWebsocketState != webSocket.isConnected()) {
    lastWebsocketState = webSocket.isConnected();
    lcdChange();
  }
}

void rfidLoop() {

  if (millis() - state.lastCardReadTime > 500 && 
      mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial())
  {
    state.lastCardReadTime = millis();
    if (state.solverCardId > 0 && state.judgeCardId > 0) return; // if both card were already scanned

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
    // Logger.printf("State changed from %c to %c\n", state.lastTiemrState, stackmat.state());
    switch (stackmat.state())
    {
      case ST_Stopped:
        if (!state.timeStarted || state.solverCardId == 0 || state.finishedSolveTime > 0) break;

        Logger.printf("FINISH! Final time is %i:%02i.%03i!\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        state.finishedSolveTime = stackmat.time();

        saveState();
        break;

      case ST_Reset:
        // Logger.println("Timer reset!");
        break;

      case ST_Running:
        if (state.solverCardId == 0 || state.finishedSolveTime > 0) break;
        state.solveSessionId++;
        state.finishedSolveTime = -1;
        state.timeOffset = 0;
        state.judgeCardId = 0;
        state.timeConfirmed = false;
        state.timeStarted = true;

        Logger.println("Solve started!");
        Logger.printf("Solve session ID: %i\n", state.solveSessionId);
        break;

      default:
        break;
    }

    lcdChange();
  }

  if (stackmat.state() == StackmatTimerState::ST_Running) {
    lcdChange();
  } else if (stackmat.connected() != state.stackmatConnected) {
    state.stackmatConnected = stackmat.connected();
    lcdChange();
  }

  state.lastTiemrState = stackmat.state();
}
