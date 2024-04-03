#ifndef __BT_HPP__
#define __BT_HPP__

#include "ws_logger.h"

// UNCOMMENT TO ENABLE BLUETOOTH WIFI SETUP!
#define BLUETOOTH_ENABLE

#ifdef BLUETOOTH_ENABLE
#include <BLEDevice.h>
#include <BLEUtils.h>
#include <BLEServer.h>

#define SERVICE_UUID "3ee59312-20bc-4c38-9e23-e785b6c385b1"
#define CHARACTERISTIC_UUID "e2ed1fc5-0d2e-4c2d-a0a7-31e38431cc0c"

class MyCharacteristicCallbacks: public BLECharacteristicCallbacks {
  void onWrite(BLECharacteristic *pCharacteristic) {
    std::string value = pCharacteristic->getValue();
    if (value.length() > 0 && WiFi.status() != WL_CONNECTED && !wifiConnected) {
      char *str = (char *)value.c_str();
        
      char *ssid = strtok_r(str, "|", &str);
      char *pass = strtok_r(str, "|", &str);

    //   Logger.printf("Characteristic written to: %s\n", value.c_str());
      Logger.printf("BT SSID: %s\n", ssid);
      Logger.printf("BT PASS: %s\n", pass);
      WiFi.begin(ssid, pass);

      delay(500);
      ESP.restart();
    }
  }
};
#endif

void initBt(char* deviceName) {
  #ifdef BLUETOOTH_ENABLE
  Logger.printf("Starting bt le handler!\n");

  BLEDevice::init(deviceName);
  BLEServer *pServer = BLEDevice::createServer();

  BLEService *pService = pServer->createService(SERVICE_UUID);
  BLECharacteristic *pCharacteristic = pService->createCharacteristic(CHARACTERISTIC_UUID, BLECharacteristic::PROPERTY_WRITE);

  pCharacteristic->setCallbacks(new MyCharacteristicCallbacks());
  pService->start();


  BLEAdvertising *pAdvertising = BLEDevice::getAdvertising();
  pAdvertising->addServiceUUID(SERVICE_UUID);
  pAdvertising->setScanResponse(true);
  pAdvertising->setMinPreferred(0x06); // functions that help with iPhone connections issue
  pAdvertising->setMinPreferred(0x12);
  BLEDevice::startAdvertising();
  #endif
}

void deinitBt(bool release_memory = false) {
  #ifdef BLUETOOTH_ENABLE
  Logger.printf("Stopping bt le handler!\n");
  BLEDevice::deinit(release_memory);
  #endif
}

#endif