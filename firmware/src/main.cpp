#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include <WiFi.h>
#include <WiFiManager.h>
#include <driver/rtc_io.h>

#include "ws_logger.h"

void setup1(void* pvParameters);
void loop1();

void sleepTest() {
  Serial.println("Going to sleep now");
  delay(1000);
  Serial.println("3");
  delay(1000);
  Serial.println("2");
  delay(1000);
  Serial.println("1");
  delay(1000);
  Serial.flush(); 

  rtc_gpio_hold_en(GPIO_NUM_33);
  esp_sleep_enable_ext0_wakeup(GPIO_NUM_33, LOW);
  esp_light_sleep_start();
}

void setup() {
  pinMode(33, INPUT_PULLUP);

  xTaskCreatePinnedToCore (
    setup1,     // Function to implement the task
    "core2",   // Name of the task
    10000,      // Stack size in words
    NULL,      // Task input parameter
    0,         // Priority of the task
    NULL,      // Task handle.
    0          // Core where the task should run
  );

  Serial.begin(115200);
  Logger.begin(&Serial);
}

void setup1(void* pvParameters) {
  

  while(1) {
    loop1();
  }
}

void loop() {
  Logger.printf("dsadsa: %d\n", 213);
  Logger.loop();

  if (digitalRead(33) == LOW) {
    sleepTest();
  }

  delay(500);
}

void loop1() {
  Serial.printf("on second core, not using logger here!\n");
  delay(1000);
}