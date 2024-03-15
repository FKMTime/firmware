#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include <WiFi.h>
#include <WiFiManager.h>

#include "ws_logger.h"
#include "pins.h"
#include "utils.hpp"

void core2(void* pvParameters);
inline void loop2();

void setup() {
  pinMode(BUTTON1, INPUT_PULLUP);
  pinMode(BUTTON2, INPUT_PULLUP);
  pinMode(BUTTON3, INPUT_PULLUP);
  pinMode(BAT_ADC, INPUT);

  Serial.begin(115200);
  Logger.begin(&Serial);

  xTaskCreatePinnedToCore(core2, "core2", 10000, NULL, 0, NULL, 1);
}

void loop() {
  if (digitalRead(BUTTON2) == LOW) {
    lightSleep(SLEEP_WAKE_BUTTON, LOW);
  }
}

void core2(void* pvParameters) {
  while(1) {
    loop2();
  }
}

inline void loop2() {

}