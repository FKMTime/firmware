#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include "soc/soc.h"
#include "soc/rtc_cntl_reg.h"

#include "ws_logger.h"
#include "pins.h"
#include "utils.hpp"
#include "version.h"
#include "globals.hpp"
#include "lcd.hpp"
#include "radio/radio.hpp"
#include <stackmat.h>

void core2(void* pvParameters);
inline void loop2();

MFRC522 mfrc522(RFID_CS, UNUSED_PIN);

void setup() {
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 0);

  Serial.begin(115200);
  Serial2.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, STACKMAT_JACK);
  Logger.begin(&Serial);
  EEPROM.begin(128);
  stackmat.begin(&Serial2);
  SPI.begin(RFID_SCK, RFID_MISO, RFID_MOSI, RFID_CS);
  Wire.begin(LCD_SDA, LCD_SCL);

  mfrc522.PCD_Init();
  lcdInit();

  pinMode(BUTTON1, INPUT_PULLUP);
  pinMode(BUTTON2, INPUT_PULLUP);
  pinMode(BUTTON3, INPUT_PULLUP);
  pinMode(BAT_ADC, INPUT);

  currentBatteryVoltage = readBatteryVoltage(BAT_ADC, 15, false);
  float initialBat = voltageToPercentage(currentBatteryVoltage);
  Logger.printf("ESP ID: %x\n", ESP.getEfuseMac());
  Logger.printf("Current firmware version: %s\n", FIRMWARE_VERSION);
  Logger.printf("Build time: %s\n", BUILD_TIME);
  Logger.printf("Battery: %f%% (%fv)\n", initialBat, currentBatteryVoltage);

  lcdPrintf(0, true, ALIGN_LEFT, "ID: %x", ESP.getEfuseMac());
  lcdPrintf(0, false, ALIGN_RIGHT, "%d%%", (int)initialBat);
  lcdPrintf(1, true, ALIGN_LEFT, "VER: %s", FIRMWARE_VERSION);

  initWifi();

  xTaskCreatePinnedToCore(core2, "core2", 10000, NULL, 0, NULL, 0);
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 1); 
}

unsigned long lastBatRead = 0;
void loop() {
  lcdLoop();
  webSocket.loop();
  Logger.loop();

  if (millis() - lastBatRead > BATTERY_READ_INTERVAL) {
    currentBatteryVoltage = readBatteryVoltage(BAT_ADC, 15, false);
    float batPerct = voltageToPercentage(currentBatteryVoltage);

    Logger.printf("Battery: %f%% (%fv)\n", batPerct, currentBatteryVoltage);
    lastBatRead = millis();
  }

  delay(5);
}

void core2(void* pvParameters) {
  while(1) {
    loop2();
  }
}

inline void loop2() {
  // if (digitalRead(BUTTON2) == LOW) {
  //   lightSleep(SLEEP_WAKE_BUTTON, LOW);
  // }
  // Serial.printf("millis: %lu\n", millis());
  delay(10);
}