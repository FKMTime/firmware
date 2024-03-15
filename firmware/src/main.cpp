#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include <WiFi.h>
#include <WiFiManager.h>

#include <BLEDevice.h>
#include <BLEUtils.h>
#include <BLEServer.h>

#include "ws_logger.h"

#define SERVICE_UUID "3ee59312-20bc-4c38-9e23-e785b6c385b1"
#define CHARACTERISTIC_UUID "e2ed1fc5-0d2e-4c2d-a0a7-31e38431cc0c"


void setup1(void* pvParameters);
void loop1();

class MyCharacteristicCallbacks: public BLECharacteristicCallbacks {
  void onWrite(BLECharacteristic *pCharacteristic) {
    std::string value = pCharacteristic->getValue();
    if (value.length() > 0) {
      Serial.printf("Characteristic written to: %s\n", value.c_str());
    }
  }
};

void setup() {
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

  BLEDevice::init("ESP32");
  BLEServer *pServer = BLEDevice::createServer();

  BLEService *pService = pServer->createService(SERVICE_UUID);
  BLECharacteristic *pCharacteristic = pService->createCharacteristic(CHARACTERISTIC_UUID, 
    BLECharacteristic::PROPERTY_READ | BLECharacteristic::PROPERTY_WRITE);

  pCharacteristic->setValue("Hello World says Neil");
  pCharacteristic->setCallbacks(new MyCharacteristicCallbacks());
  pService->start();


  BLEAdvertising *pAdvertising = BLEDevice::getAdvertising();
  pAdvertising->addServiceUUID(SERVICE_UUID);
  pAdvertising->setScanResponse(true);
  pAdvertising->setMinPreferred(0x06); // functions that help with iPhone connections issue
  pAdvertising->setMinPreferred(0x12);
  BLEDevice::startAdvertising();
}

void setup1(void* pvParameters) {
  

  while(1) {
    loop1();
  }
}

void loop() {
  Logger.printf("dsadsa: %d\n", 213);
  Logger.loop();
  delay(500);
}

void loop1() {
  Serial.printf("on second core, not using logger here!\n");
  delay(1000);
}