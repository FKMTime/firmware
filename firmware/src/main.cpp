#include <Arduino.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include <Wire.h>
#include "soc/soc.h"
#include "soc/rtc_cntl_reg.h"

#include "ws_logger.h"
#include "pins.h"
#include "utils.hpp"
#include "version.h"
#include "globals.hpp"
#include "lcd.hpp"
#include "buttons.hpp"
#include "state.hpp"
#include "radio/radio.hpp"
#include <stackmat.h>

void core2(void *pvParameters);
inline void loop2();
void rfidLoop();
void sleepDetection();
void stackmatLoop();

void setup() {
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 0);

  pinMode(BUTTON1, INPUT_PULLUP);
  pinMode(BUTTON2, INPUT_PULLUP);
  pinMode(BUTTON3, INPUT_PULLUP);
  pinMode(BUTTON4, INPUT_PULLUP);
  pinMode(BAT_ADC, INPUT);

  Serial.begin(115200);
  Logger.begin(&Serial);
  EEPROM.begin(128);
  Serial1.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, STACKMAT_JACK, 255, false);
  stackmat.begin(&Serial1);
  SPI.begin(RFID_SCK, RFID_MISO, RFID_MOSI);
  Wire.begin(LCD_SDA, LCD_SCL);

  buttonsInit();
  mfrc522.PCD_Init();
  lcdInit();

  delay(100);
  currentBatteryVoltage = readBatteryVoltage(BAT_ADC, 15, false);
  float initialBat = voltageToPercentage(currentBatteryVoltage);
  Logger.printf("ESP ID: %x\n", (unsigned long)ESP.getEfuseMac());
  Logger.printf("Current firmware version: %s\n", FIRMWARE_VERSION);
  Logger.printf("Build time: %s\n", BUILD_TIME);
  Logger.printf("Battery: %f%% (%fv)\n", initialBat, currentBatteryVoltage);

  lcdPrintf(0, true, ALIGN_LEFT, "ID: %x", (unsigned long)ESP.getEfuseMac());
  lcdPrintf(0, false, ALIGN_RIGHT, "%d%%", (int)initialBat);
  lcdPrintf(1, true, ALIGN_LEFT, "VER: %s", FIRMWARE_VERSION);

  initWifi();
  lcdClear();

  // IDK WHY BUT MY STACKMAT IMPLEMENTATION IS BUGGY WHEN THERE IS A LOT OF 
  // DATA INSIDE Serial BUFFER, SO IT WILL FIX IT
  while(Serial1.available()) { Serial1.read(); } 

  initState();
  xTaskCreatePinnedToCore(core2, "core2", 10000, NULL, 0, NULL, 0);
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 1);
}

unsigned long lastBatRead = 0;
void loop() {
  lcdStateManagementLoop(); // non blocking
  lcdPrintLoop();           // non blocking
  Logger.loop();            // non blocking
  webSocket.loop();         // non blocking
  stackmat.loop();          // non blocking
  stackmatLoop();           // non blocking

  sleepDetection();

  // non blocking
  // TODO: maybe move this into own function?
  if (millis() - lastBatRead > BATTERY_READ_INTERVAL) {
    currentBatteryVoltage = readBatteryVoltage(BAT_ADC, 15, false);
    float batPerct = voltageToPercentage(currentBatteryVoltage);

    // TODO: remove this battery log
    Logger.printf("Battery: %f%% (%fv)\n", batPerct, currentBatteryVoltage);
    sendBatteryStats(batPerct, currentBatteryVoltage);
    lastBatRead = millis();
  }

  delay(5);
}

void core2(void *pvParameters) {
  while (1) {
    loop2();
  }
}

inline void loop2() {
  rfidLoop();     // blocking (when card is close to scanner)
  buttons.loop(); // blocking

  delay(10);
}

// TODO: maybe move it into own file?
unsigned long lastCardReadTime = 0;
void rfidLoop() {
  if (millis() - lastCardReadTime < 500) return;
  if (!mfrc522.PICC_IsNewCardPresent()) return;
  if (!mfrc522.PICC_ReadCardSerial()) return;

  unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
  Logger.printf("Scanned card ID: %lu\n", cardId);

  JsonDocument doc;
  doc["card_info_request"]["card_id"] = cardId;
  doc["card_info_request"]["esp_id"] = (unsigned long)ESP.getEfuseMac();

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);

  if(!webSocket.isConnected()) {
    showError("Server not connected!");
  }

  mfrc522.PICC_HaltA();
  lastCardReadTime = millis();
}

void sleepDetection() {
  unsigned long timeSinceLastDraw = millis() - lcdLastDraw;
  if (timeSinceLastDraw > SLEEP_TIME && !lcdHasChanged) {
    lcdPrintf(0, true, ALIGN_CENTER, "Sleep mode");
    lcdPrintf(1, true, ALIGN_CENTER, "Turn on timer");
    lcd.noBacklight();
    mfrc522.PCD_SoftPowerDown();

    // enter light sleep and wait for SLEEP_WAKE_BUTTON to be pressed
    lightSleep(SLEEP_WAKE_BUTTON, LOW);

    lcd.backlight();
    lcdClear();
    stateHasChanged = true;

    WiFi.disconnect();
    WiFi.reconnect();
    mfrc522.PCD_SoftPowerUp();

    return;
  }
}

void stackmatLoop() {
  if (stackmat.state() != state.lastTimerState && stackmat.state() != ST_Unknown && state.lastTimerState != ST_Unknown) {
    // Logger.printf("State changed from %c to %c\n", state.lastTimerState, stackmat.state());
    switch (stackmat.state()) {
      case ST_Stopped:
        if (state.competitorCardId == 0 || state.solveTime > 0) break;

        Logger.printf("FINISH! Final time is %i:%02i.%03i!\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        startSolveSession(stackmat.time());
        break;

      case ST_Reset:
        Logger.println("Timer reset!");
        break;

      case ST_Running:
        if (state.competitorCardId == 0 || state.solveTime > 0) break;

        #ifdef INSPECTION_ENABLE
        stopInspection();
        #endif

        state.currentScene = SCENE_TIMER_TIME;
        Logger.println("Solve started!");
        break;

      default:
        break;
    }

    stateHasChanged = true;
  }

  if (stackmat.state() == StackmatTimerState::ST_Running && state.currentScene == SCENE_TIMER_TIME) {
    lcdPrintf(0, true, ALIGN_CENTER, "%s", displayTime(stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds()).c_str());
    lcdClearLine(1);
  } else if (stackmat.connected() != lastStackmatConnected) {
    lastStackmatConnected = stackmat.connected();
    stateHasChanged = true;
  }

  state.lastTimerState = stackmat.state();
}
