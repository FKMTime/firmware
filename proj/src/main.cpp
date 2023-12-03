#include <Arduino.h>
#include <ESP8266WiFi.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>
#include "rgb_lcd.h"

#define RST_PIN A0
#define SS_PIN 16
#define SCK_PIN 14
#define MISO_PIN 12
#define MOSI_PIN 13

#define STACKMAT_TIMER_PIN D5

#define OK_BUTTON_PIN D9
#define PLUS2_BUTTON_PIN D1
#define DNF_BUTTON_PIN D0

#define STACKMAT_TIMER_BAUD_RATE 1200
#define STACKMAT_TIMER_TIMEOUT 1000

String getChipID();
MFRC522 mfrc522(SS_PIN, RST_PIN);
rgb_lcd lcd;

void setup() {
  Serial.begin(115200);
  pinMode(2, INPUT_PULLUP);
  pinMode(15, INPUT_PULLUP);

  SPI.pins(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  SPI.begin();
  mfrc522.PCD_Init();

  lcd.begin(16, 2);
  lcd.clear();

  lcd.print("ID: ");
  lcd.setCursor(0, 1);
  lcd.print(getChipID());

}

void loop() {
  Serial.println(digitalRead(15));
  Serial.println(analogRead(15));

  delay(100);
  if (mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial()) {
    unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
    Serial.print("Card ID: ");
    Serial.println(cardId);

    lcd.setCursor(0, 1);
    lcd.print("               ");
    lcd.setCursor(0, 1);
    lcd.printf("ID: %lu", cardId);
  }
}

String getChipID() {
  uint64_t chipid = ESP.getChipId();
  String chipidStr = String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
  return chipidStr;
}