#ifndef __GLOBALS_HPP__
#define __GLOBALS_HPP__

#include <Arduino.h>
#include <LiquidCrystal_I2C.h>
#include <MFRC522v2.h>
#include <MFRC522DriverSPI.h>
#include <MFRC522DriverPinSimple.h>
#include <MFRC522Debug.h>
#include <stackmat.h>
#include "defines.h"

float currentBatteryVoltage = 0.0;
bool primaryLangauge = false; // primary language is EN so non primary is PL

LiquidCrystal_I2C lcd(LCD_ADDR, LCD_SIZE_X, LCD_SIZE_Y);
WebSocketsClient webSocket;
Stackmat stackmat;

// RFID
MFRC522DriverPinSimple ss_pin(RFID_CS);
MFRC522DriverSPI driver{ss_pin};
MFRC522 mfrc522{driver};

#endif