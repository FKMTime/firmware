#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include <WiFi.h>
#include <WiFiManager.h>

#include "ws_logger.h"

void setup() {
  Serial.begin(115200);
  Logger.begin(&Serial);
}

void loop() {
  Logger.printf("dsadsa: %d\n", 213);
  Logger.loop();
  delay(500);
}