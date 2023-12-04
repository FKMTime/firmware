#include <Arduino.h>
#include <ESP8266WiFi.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>

#include "stackmat.h"
#include "rgb_lcd.h"

#define RST_PIN A0
#define SS_PIN 16
#define SCK_PIN 14
#define MISO_PIN 12
#define MOSI_PIN 13
#define STACKMAT_TIMER_PIN 1
#define OK_BUTTON_PIN D9
#define PLUS2_BUTTON_PIN D1
#define DNF_BUTTON_PIN D0

String getChipID();
void stackmatReader();
MFRC522 mfrc522(SS_PIN, RST_PIN);
rgb_lcd lcd;
Stackmat stackmat;

StackmatTimerState currentTimerState = ST_Reset;
StackmatTimerState lastTimerState = ST_Unknown;

int solveSessionId = 0;
unsigned long lastCardReadTime = 0;

int lastTimerTime = 0;
int finishedSolveTime = 0;
int timerOffset = 0;

bool timeConfirmed = false;
bool isConnected = false;
bool lastIsConnected = false;

void setup() {
  Serial.pins(255, 3);
  Serial.begin(115200);

  Serial1.pins(255, STACKMAT_TIMER_PIN);
  Serial1.begin(STACKMAT_TIMER_BAUD_RATE);
  stackmat.begin(&Serial1);

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
  // Serial.println(digitalRead(15));
  // Serial.println(analogRead(15));

  delay(20);
  if (mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial()) {
    unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
    Serial.print("Card ID: ");
    Serial.println(cardId);

    lcd.setCursor(0, 1);
    lcd.print("               ");
    lcd.setCursor(0, 1);
    lcd.printf("ID: %lu", cardId);
  }

  stackmatReader();
}

String getChipID() {
  uint64_t chipid = ESP.getChipId();
  String chipidStr = String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
  return chipidStr;
}

void stackmatReader() {
  isConnected = millis() - lastUpdated < STACKMAT_TIMER_TIMEOUT;
  if (isConnected) {
    if (!lastIsConnected) {
      lcd.clear();
      lcd.setCursor(0, 0);
      lcd.print("Stackmat Timer");
      lcd.setCursor(0, 1);
      lcd.print("Connected");
    }

    if (currentTimerState != lastTimerState && currentTimerState != ST_Unknown && lastTimerState != ST_Unknown) {
      Serial.printf("State changed from %c to %c\n", lastTimerState, currentTimerState);
      switch (currentTimerState) {
        case ST_Stopped:
          Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
          finishedSolveTime = timerTime;
          lastTimerTime = timerTime;

          lcd.clear();
          lcd.setCursor(0, 0);
          lcd.printf("TIME: %i:%02i.%03i", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());

          //webSocket.sendTXT("{\"time\": " + String(timerTime) + "}");
          // writeEEPROMInt(4, finishedSolveTime);
          // EEPROM.commit();
          break;
        case ST_Reset:
          Serial.println("Timer is reset!");
          break;
        case ST_Running:
          solveSessionId++;

          Serial.println("Solve started!");
          Serial.printf("Solve session ID: %i\n", solveSessionId);
          // writeEEPROMInt(0, solveSessionId);
          break;
        default:
          break;
      }
    }

    if (currentTimerState == ST_Running) {
      if (timerTime != lastTimerTime) {
        Serial.printf("%i:%02i.%03i\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
        lcd.clear();
        lcd.setCursor(0, 0);
        lcd.printf("TIME: %i:%02i.%03i", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
        //webSocket.sendTXT("{\"time\": " + String(timerTime) + "}");
        lastTimerTime = timerTime;
      }
    }

    lastTimerState = currentTimerState;
  } else {
    if (lastIsConnected) {
      lcd.clear();
      lcd.setCursor(0, 0);
      lcd.print("Stackmat Timer");
      lcd.setCursor(0, 1);
      lcd.print("Disconnected");
    }
  }

  lastIsConnected = isConnected;
}