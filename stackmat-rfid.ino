#include <Arduino.h>
#include <WiFi.h>
#include <HTTPClient.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include "rgb_lcd.h"

#define RST_PIN D6
#define SS_PIN D2
#define SCK_PIN D8
#define MISO_PIN D3
#define MOSI_PIN D10

#define OK_BUTTON_PIN D9
#define PLUS2_BUTTON_PIN D1
#define DNF_BUTTON_PIN D0

#define STACKMAT_TIMER_BAUD_RATE 1200
#define STACKMAT_TIMER_TIMEOUT 1000

MFRC522 mfrc522(SS_PIN, RST_PIN);
HTTPClient https;
WebSocketsClient webSocket;
rgb_lcd lcd;

enum StackmatTimerState {
  ST_Unknown = 0,
  ST_Reset = 'I',
  ST_Running = ' ',
  ST_Stopped = 'S'
};

StackmatTimerState currentTimerState = ST_Reset;
StackmatTimerState lastTimerState = ST_Unknown;

int solveSessionId = 0;
unsigned long lastUpdated = 0;
unsigned long lastCardReadTime = 0;

int timerTime = 0;
int lastTimerTime = 0;
int finishedSolveTime = 0;
int timerOffset = 0;

bool timeConfirmed = false;
bool isConnected = false;
bool lastIsConnected = false;

void setup() {
  pinMode(OK_BUTTON_PIN, INPUT_PULLUP);
  pinMode(PLUS2_BUTTON_PIN, INPUT_PULLUP);
  pinMode(DNF_BUTTON_PIN, INPUT_PULLUP);

  Serial.begin(115200);
  Serial0.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, -1, 255, true);
  SPI.begin(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  mfrc522.PCD_Init();
  EEPROM.begin(512);

  lcd.begin(16, 2);
  lcd.clear();

  lcd.print("ID: ");
  lcd.setCursor(0, 1);
  lcd.print(getESP32ChipID());

  WiFiManager wm;
  //wm.resetSettings();

  String generatedSSID = "StackmatTimer-" + getESP32ChipID();
  wm.setConfigPortalTimeout(300);
  bool res = wm.autoConnect(generatedSSID.c_str(), "StackmatTimer");
  if (!res) {
    Serial.println("Failed to connect");
    delay(1000);
    ESP.restart();
  }

  lcd.clear();
  lcd.print("WiFi connected!");
  lcd.setCursor(0, 1);
  String ipString = String(WiFi.localIP()[0]) + "." + String(WiFi.localIP()[1]) + "." + String(WiFi.localIP()[2]) + "." + String(WiFi.localIP()[3]);
  lcd.print(ipString);

  //webSocket.beginSSL("gate.filipton.online", 443, "/");
  webSocket.begin("192.168.1.38", 8080, "/");
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);
  webSocket.sendTXT("Hello from ESP32!");

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
  Serial0.flush();

  solveSessionId = EEPROM.readInt(0);
  finishedSolveTime = EEPROM.readInt(4);

  Serial.printf("Solve session ID: %i\n", solveSessionId);
  Serial.printf("Saved finished solve time: %i\n", finishedSolveTime);
}

void loop() {
  webSocket.loop();
  cardReader();
  stackmatReader();

  if (digitalRead(OK_BUTTON_PIN) == LOW) {
    Serial.println("OK button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(OK_BUTTON_PIN) == LOW) {
      delay(10);
    }

    if (millis() - pressedTime > 5000) {
      Serial.println("Resettings finished solve time!");
      finishedSolveTime = 0;
      timeConfirmed = false;
    } else {
      timeConfirmed = true;
    }
  }

  if (digitalRead(PLUS2_BUTTON_PIN) == LOW) {
    Serial.println("+2 button pressed!");
    //unsigned long pressedTime = millis();
    while (digitalRead(PLUS2_BUTTON_PIN) == LOW) {
      delay(10);
    }

    if (timerOffset != -1) {
      timerOffset = timerOffset >= 16 ? 0 : timerOffset + 2;
    }
  }

  if (digitalRead(DNF_BUTTON_PIN) == LOW) {
    Serial.println("DNF button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(DNF_BUTTON_PIN) == LOW) {
      delay(10);
    }

    if (millis() - pressedTime > 10000) {
      Serial.println("Resetting wifi settings!");
      WiFiManager wm;
      wm.resetSettings();
      delay(1000);
      ESP.restart();
    } else {
      timerOffset = timerOffset != -1 ? -1 : 0;
    }
  }

  Serial.printf("Timer offset: %i\n", timerOffset);
}

void cardReader() {
  if (millis() - lastCardReadTime > 1000 && mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial()) {
    if (finishedSolveTime == 0) {
      Serial.println("Please start the timer before scanning a card!");
      return;
    }

    if (!timeConfirmed) {
      Serial.println("Please confirm the solve by pressing OK button!");
      return;
    }

    if (currentTimerState == ST_Running) {
      Serial.println("Solve is running! Please stop the timer before scanning a new card.");
      return;
    }

    unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
    Serial.print("Card ID: ");
    Serial.println(cardId);

    lcd.setCursor(0, 1);
    lcd.print("               ");
    lcd.setCursor(0, 1);
    lcd.printf("ID: %lu", cardId);

    struct tm timeinfo;
    if (!getLocalTime(&timeinfo)) {
      Serial.println("Failed to obtain time");
    }
    time_t epoch;
    time(&epoch);

    DynamicJsonDocument doc(256);
    doc["cardId"] = cardId;
    doc["solveTime"] = finishedSolveTime;
    doc["espId"] = getESP32ChipID();
    doc["timestamp"] = epoch;
    doc["solveSessionId"] = solveSessionId;

    String json;
    serializeJson(doc, json);

    webSocket.sendTXT(json);
    lastCardReadTime = millis();

    // This should be done after the solve is sent to the server (after the server responds)
    /*
    currentState = S_NotConfirmed;
    finishedSolveTime = 0;
    */
  }
}

void stackmatReader() {
  String data;
  while (Serial0.available() > 9) {
    data = readStackmatString();

    if (data.length() >= 8) {
      ParseTimerData(data);
    }
  }

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
          EEPROM.writeInt(4, finishedSolveTime);
          EEPROM.commit();
          break;
        case ST_Reset:
          Serial.println("Timer is reset!");
          break;
        case ST_Running:
          solveSessionId++;

          Serial.println("Solve started!");
          Serial.printf("Solve session ID: %i\n", solveSessionId);
          EEPROM.writeInt(0, solveSessionId);
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

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
  if (type == WStype_TEXT) {
    DynamicJsonDocument doc(2048);
    deserializeJson(doc, payload);

    Serial.printf("Received message: %s\n", doc["espId"].as<const char *>());
  } else if (type == WStype_CONNECTED) {
    Serial.println("Connected to WebSocket server");
  } else if (type == WStype_DISCONNECTED) {
    Serial.println("Disconnected from WebSocket server");
  }
}

String getESP32ChipID() {
  uint64_t chipid = ESP.getEfuseMac();
  String chipidStr = String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
  return chipidStr;
}

String readStackmatString() {
  unsigned long startTime = millis();
  String tmp;

  while (millis() - startTime < 1000) {
    if (Serial0.available() > 0) {
      char c = Serial0.read();
      if ((int)c == 0) {
        return tmp;
      }

      if (c == '\r') {
        return tmp;
      }

      tmp += c;
      startTime = millis();
    }
  }

  return "";
}

bool ParseTimerData(String data) {
  StackmatTimerState state = (StackmatTimerState)data[0];
  if (data[0] != 'I' && data[0] != ' ' && data[0] != 'S') {
    state = ST_Unknown;
  }

  int minutes = data.substring(1, 2).toInt();
  int seconds = data.substring(2, 4).toInt();
  int ms = data.substring(4, 7).toInt();
  int cheksum = (int)data[7];

  int totalMs = ms + (seconds * 1000) + (minutes * 60 * 1000);
  int sum = 64;
  for (int i = 0; i < 7; i++) {
    sum += data.substring(i, i + 1).toInt();
  }

  if (sum != cheksum) {
    return false;
  }

  if (totalMs > 0 && state == ST_Reset) {
    state = ST_Stopped;
  }

  currentTimerState = state;
  lastUpdated = millis();
  timerTime = totalMs;

  return true;
}

uint8_t GetDisplayMinutes() {
  return timerTime / 60000;
}
uint8_t GetDisplaySeconds() {
  return (timerTime - ((timerTime / 60000) * 60000)) / 1000;
}
uint16_t GetDisplayMilliseconds() {
  uint32_t time = timerTime;
  time -= ((time / 60000) * 60000);
  time -= ((time / 1000) * 1000);
  return time;
}
