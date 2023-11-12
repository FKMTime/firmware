#include <Arduino.h>
#include <WiFi.h>
#include <HTTPClient.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>
#include <SPI.h>
#include <MFRC522.h>
#include <EEPROM.h>
#include <ArduinoJson.h>

#define RST_PIN D6
#define SS_PIN D4
#define SCK_PIN D8
#define MISO_PIN D5
#define MOSI_PIN D10

#define STACKMAT_TIMER_BAUD_RATE 1200
#define STACKMAT_TIMER_TIMEOUT 1000

MFRC522 mfrc522(SS_PIN, RST_PIN);
HTTPClient https;
WebSocketsClient webSocket;

enum StackmatTimerState {
  ST_Unknown = 0,
  ST_Reset = 'I',
  ST_Running = ' ',
  ST_Stopped = 'S'
};

StackmatTimerState currentState = ST_Reset;
StackmatTimerState lastState = ST_Unknown;

unsigned long solveSessionId = 0;
unsigned long lastUpdated = 0;
unsigned long lastCardReadTime = 0;

unsigned long timerTime = 0;
unsigned long lastTimerTime = 0;
unsigned long finishedSolveTime = 0;

bool isConnected = false;

void setup() {
  pinMode(D3, OUTPUT);
  Serial.begin(115200);
  Serial0.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, -1, 255, true);
  SPI.begin(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  mfrc522.PCD_Init();
  EEPROM.begin(512);

  digitalWrite(D3, HIGH);
  delay(500);
  digitalWrite(D3, LOW);

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

  Serial.print("IP address: ");
  Serial.println(WiFi.localIP());

  digitalWrite(D3, HIGH);
  delay(25);
  digitalWrite(D3, LOW);
  delay(25);
  digitalWrite(D3, HIGH);
  delay(25);
  digitalWrite(D3, LOW);

  //webSocket.beginSSL("gate.filipton.online", 443, "/");
  webSocket.begin("192.168.1.38", 8080, "/");
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);
  webSocket.sendTXT("Hello from ESP32!");

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
  Serial0.flush();

  solveSessionId = EEPROM.readULong(0);
  finishedSolveTime = EEPROM.readULong(4);

  Serial.printf("Solve session ID: %i\n", solveSessionId);
  Serial.printf("Saved finished solve time: %i\n", finishedSolveTime);
}

void loop() {
  webSocket.loop();

  if (millis() - lastCardReadTime > 1000 && mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial()) {
    if (currentState == ST_Running) {
      Serial.println("Solve is running! Please stop the timer before scanning a new card.");
      return;
    }

    unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
    Serial.print("Card ID: ");
    Serial.println(cardId);

    struct tm timeinfo;
    if (!getLocalTime(&timeinfo)) {
      Serial.println("Failed to obtain time");
    }
    time_t epoch;
    time(&epoch);

    digitalWrite(D3, HIGH);
    delay(50);
    digitalWrite(D3, LOW);
    delay(50);

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
  }

  String data;
  while (Serial0.available() > 9) {
    data = readStackmatString();

    if (data.length() >= 8) {
      ParseTimerData(data);
    }
  }

  isConnected = millis() - lastUpdated < STACKMAT_TIMER_TIMEOUT;
  if (isConnected) {
    if (currentState != lastState && currentState != ST_Unknown && lastState != ST_Unknown) {
      Serial.printf("State changed from %c to %c\n", lastState, currentState);
      switch (currentState) {
        case ST_Stopped:
          Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
          finishedSolveTime = timerTime;
          lastTimerTime = timerTime;
          webSocket.sendTXT("{\"time\": " + String(timerTime) + "}");

          EEPROM.writeULong(4, finishedSolveTime);
          EEPROM.commit();
          break;
        case ST_Reset:
          Serial.println("Timer is reset!");
          break;
        case ST_Running:
          solveSessionId++;

          Serial.println("Solve started!");
          Serial.printf("Solve session ID: %i\n", solveSessionId);
          EEPROM.writeULong(0, solveSessionId);
          break;
        default:
          break;
      }
    }

    if (currentState == ST_Running) {
      if (timerTime != lastTimerTime) {
        Serial.printf("%i:%02i.%03i\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
        webSocket.sendTXT("{\"time\": " + String(timerTime) + "}");
        lastTimerTime = timerTime;
      }
    }

    lastState = currentState;
  }
}

void blinkDebugLed(int times, int delayTime) {
  for (int i = 0; i < times; i++) {
    digitalWrite(D3, HIGH);
    delay(delayTime);
    digitalWrite(D3, LOW);
    delay(delayTime);
  }
}

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
  if (type == WStype_TEXT) {
    DynamicJsonDocument doc(2048);
    deserializeJson(doc, payload);

    Serial.printf("Received message: %s\n", doc["espId"].as<const char*>());
  } else if (type == WStype_CONNECTED) {
    Serial.println("Connected to WebSocket server");
    blinkDebugLed(4, 50);
  } else if (type == WStype_DISCONNECTED) {
    Serial.println("Disconnected from WebSocket server");
    blinkDebugLed(2, 250);
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

  unsigned long totalMs = ms + (seconds * 1000) + (minutes * 60 * 1000);
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

  currentState = state;
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
