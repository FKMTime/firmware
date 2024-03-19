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

void setup() {
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 0);

  pinMode(BUTTON1, INPUT_PULLUP);
  pinMode(BUTTON2, INPUT_PULLUP);
  pinMode(BUTTON3, INPUT_PULLUP);
  pinMode(BAT_ADC, INPUT);

  Serial.begin(115200);
  Serial2.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, STACKMAT_JACK);
  Logger.begin(&Serial);
  EEPROM.begin(128);
  stackmat.begin(&Serial2);
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

  mfrc522.PICC_HaltA();
  lastCardReadTime = millis();
}

void sleepDetection() {
  unsigned long timeSinceLastDraw = millis() - lcdLastDraw;
  if (timeSinceLastDraw > SLEEP_TIME && !lcdHasChanged) {
    lcdPrintf(0, true, ALIGN_CENTER, "Sleep mode");
    lcdPrintf(1, true, ALIGN_CENTER, "Submit to wake");
    lcd.noBacklight();
    mfrc522.PCD_SoftPowerDown();

    // enter light sleep and wait for SLEEP_WAKE_BUTTON to be pressed
    lightSleep(SLEEP_WAKE_BUTTON, LOW);

    lcd.backlight();
    lcdClear();
    lcdHasChanged = true;

    // refresh state (not done yet)

    WiFi.disconnect();
    WiFi.reconnect();
    mfrc522.PCD_SoftPowerUp();

    return;
  }
}