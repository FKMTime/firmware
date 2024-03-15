#ifndef __GLOBALS_HPP__
#define __GLOBALS_HPP__

#include <Arduino.h>
#include <LiquidCrystal_I2C.h>
#include <stackmat.h>
#include "defines.h"

LiquidCrystal_I2C lcd(LCD_ADDR, LCD_SIZE_X, LCD_SIZE_Y);
Stackmat stackmat;

#endif