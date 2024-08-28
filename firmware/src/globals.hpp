#ifndef __GLOBALS_HPP__
#define __GLOBALS_HPP__

#include "defines.h"
#include "pins.h"
#include <Arduino.h>
#include <LiquidCrystal_I2C.h>
#include <MFRC522Debug.h>
#include <MFRC522DriverPinSimple.h>
#include <MFRC522DriverSPI.h>
#include <MFRC522v2.h>
#include <WebSocketsClient.h>
#include <stackmat.h>

extern float currentBatteryVoltage;
extern bool wifiConnected;
extern bool primaryLangauge;

extern LiquidCrystal_I2C lcd;
extern WebSocketsClient webSocket;
extern Stackmat stackmat;

// RFID
extern MFRC522DriverPinSimple ss_pin;
extern MFRC522DriverSPI driver;
extern MFRC522 mfrc522;

/// Get esp id as uint32_t (only 31bits)
unsigned long getEspId();

#endif
