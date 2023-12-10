#include <Arduino.h>
#include <ESP8266WiFi.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>
#include <SoftwareSerial.h>

#include "utils.hpp"
#include "stackmat.h"
#include "rgb_lcd.h"

#define RST_PIN A0
#define SS_PIN 16
#define SCK_PIN 14
#define MISO_PIN 12
#define MOSI_PIN 13
#define STACKMAT_TIMER_PIN 2
#define OK_BUTTON_PIN 3
#define PLUS2_BUTTON_PIN 15
#define DNF_BUTTON_PIN 0

void stackmatReader();
void lcdLoop();

SoftwareSerial stackmatSerial(STACKMAT_TIMER_PIN, -1, true);
MFRC522 mfrc522(SS_PIN, RST_PIN);
WebSocketsClient webSocket;
Stackmat stackmat;
rgb_lcd lcd;

StackmatTimerState lastTimerState = ST_Unknown;

int solveSessionId = 0;
unsigned long lastCardReadTime = 0;

int lastTimerTime = 0;
int finishedSolveTime = 0;
int timerOffset = 0;

bool timeConfirmed = false;
bool lastIsConnected = false;

void setup()
{
  Serial.begin(115200, SERIAL_8N1, SERIAL_TX_ONLY, 1);

  stackmatSerial.begin(1200);
  stackmat.begin(&stackmatSerial);

  pinMode(3, INPUT_PULLUP);
  pinMode(15, INPUT_PULLUP);

  SPI.pins(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  SPI.begin();
  mfrc522.PCD_Init();

  lcd.begin(16, 2);
  lcd.clear();

  lcd.print("ID: ");
  lcd.setCursor(0, 0);
  lcd.print(getChipID());
  lcd.setCursor(0, 1);
  lcd.print("Connecting...");

  WiFiManager wm;
  // wm.resetSettings();

  String generatedSSID = "StackmatTimer-" + getChipID();
  wm.setConfigPortalTimeout(300);
  bool res = wm.autoConnect(generatedSSID.c_str(), "StackmatTimer");
  if (!res)
  {
    Serial.println("Failed to connect to wifi... Restarting!");
    delay(1500);
    ESP.restart();

    return;
  }

  lcd.clear();
  lcd.setCursor(0, 0);
  lcd.print("WiFi connected!");

  String ipString = String(WiFi.localIP()[0]) + "." + String(WiFi.localIP()[1]) + "." + String(WiFi.localIP()[2]) + "." + String(WiFi.localIP()[3]);
  lcd.print(ipString);

  // webSocket.begin("192.168.1.38", 8080, "/");
  // webSocket.onEvent(webSocketEvent);
  // webSocket.setReconnectInterval(5000);
  // webSocket.sendTXT("TODO: init msg");

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
}

void loop()
{
  stackmat.loop();
  lcdLoop();

  delay(20);
  if (mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial())
  {
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

void lcdLoop()
{
}

void stackmatReader()
{
  if (stackmat.connected())
  {
    if (!lastIsConnected)
    {
      lcd.clear();
      lcd.setCursor(0, 0);
      lcd.print("Stackmat Timer");
      lcd.setCursor(0, 1);
      lcd.print("Connected");
    }

    if (stackmat.state() != lastTimerState && stackmat.state() != ST_Unknown && lastTimerState != ST_Unknown)
    {
      Serial.printf("State changed from %c to %c\n", lastTimerState, stackmat.state());
      switch (stackmat.state())
      {
      case ST_Stopped:
        Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        finishedSolveTime = stackmat.time();
        lastTimerTime = stackmat.time();

        lcd.clear();
        lcd.setCursor(0, 0);
        lcd.printf("TIME: %i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());

        // webSocket.sendTXT("{\"time\": " + String(timerTime) + "}");
        //  writeEEPROMInt(4, finishedSolveTime);
        //  EEPROM.commit();
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

    if (stackmat.state() == ST_Running)
    {
      if (stackmat.time() != lastTimerTime)
      {
        Serial.printf("%i:%02i.%03i\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        lcd.clear();
        lcd.setCursor(0, 0);
        lcd.printf("TIME: %i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        // webSocket.sendTXT("{\"time\": " + String(timerTime) + "}");
        lastTimerTime = stackmat.time();
      }
    }

    lastTimerState = stackmat.state();
  }
  else
  {
    if (lastIsConnected)
    {
      lcd.clear();
      lcd.setCursor(0, 0);
      lcd.print("Stackmat Timer");
      lcd.setCursor(0, 1);
      lcd.print("Disconnected");
    }
  }

  lastIsConnected = stackmat.connected();
}