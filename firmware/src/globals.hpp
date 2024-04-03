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
bool wifiConnected = false;
bool primaryLangauge = false; // primary language is EN so non primary is PL

LiquidCrystal_I2C lcd(LCD_ADDR, LCD_SIZE_X, LCD_SIZE_Y);
WebSocketsClient webSocket;
Stackmat stackmat;

// RFID
MFRC522DriverPinSimple ss_pin(RFID_CS);
MFRC522DriverSPI driver{ss_pin};
MFRC522 mfrc522{driver};

// Get esp id as uint32_t (only 31bits)
unsigned long getEspId() {
    uint64_t efuse = ESP.getEfuseMac();
    efuse = (~efuse) + (efuse << 18);
    efuse = efuse ^ (efuse >> 31);
    efuse = efuse * 21;
    efuse = efuse ^ (efuse >> 11);
    efuse = efuse + (efuse << 6);
    efuse = efuse ^ (efuse >> 22);

    return (unsigned long)(efuse & 0x000000007FFFFFFF);
}

#endif