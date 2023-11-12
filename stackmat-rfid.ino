#include <Arduino.h>
#include <WiFi.h>
#include <HTTPClient.h>
#include <WiFiManager.h>
#include <WebSocketsClient.h>

#include <SPI.h>
#include <MFRC522.h>

#define RST_PIN D6
#define SS_PIN D4

MFRC522 mfrc522(SS_PIN, RST_PIN);
HTTPClient https;
WebSocketsClient webSocket;

static const long STACKMAT_TIMER_BAUD_RATE = 1200;
static const long STACKMAT_TIMER_TIMEOUT = 1000;

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
unsigned long timerTime = 0;
bool isConnected = false;

unsigned long finishedSolveTime = 0;

void setup() {
  pinMode(D3, OUTPUT);
  Serial.begin(115200);
  Serial0.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, -1, 255, true);
  digitalWrite(D3, HIGH);
  delay(100);
  digitalWrite(D3, LOW);
  delay(100);

  SPI.begin(D8, D5, D10, SS_PIN);  // FIXES SPI WEIRDNESS (D9 is connected as bootloader pin, so it is not usable in this case)
  mfrc522.PCD_Init();

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

  Serial.println("");
  Serial.println("WiFi connected");
  Serial.println("IP address: ");
  Serial.println(WiFi.localIP());

  /*
  if (https.begin("https://echo.filipton.space/r15578016868097582246")) {
    https.addHeader("Content-Type", "text/plain");
    int httpCode = https.POST("Startup!");
    String payload = https.getString();
    Serial.println(httpCode);
    Serial.println(payload);
  } else {
    Serial.println("Unable to connect");
  }
  */

  //webSocket.beginSSL("gate.filipton.online", 443, "/");
  webSocket.begin("192.168.1.38", 8080, "/");
  webSocket.onEvent(webSocketEvent);
  webSocket.setReconnectInterval(5000);
  webSocket.sendTXT("Hello from ESP32!");

  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
  Serial0.flush();
}

unsigned long lastCardReadTime = 0;
void loop() {
  webSocket.loop();

  if (millis() - lastCardReadTime > 1000 && mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial()) {
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

    String json = "{\"cardId\": " + String(cardId) + ", \"solveTime\": " + String(finishedSolveTime) + ", \"espId\": \"" + getESP32ChipID() + ", \"timestamp\": " + String(epoch) + "}";

    webSocket.sendTXT(json);
    /*
    if (https.begin("https://echo.filipton.space/r15578016868097582246")) {
      https.addHeader("Content-Type", "text/plain");
      https.setTimeout(3000);
      https.setConnectTimeout(3000);
      int httpCode = https.POST(json);
      String payload = https.getString();
      Serial.println(httpCode);
      Serial.println(payload);

      if (httpCode == 200) {
        digitalWrite(D3, HIGH);
        delay(50);
        digitalWrite(D3, LOW);
        delay(50);
        digitalWrite(D3, HIGH);
        delay(50);
        digitalWrite(D3, LOW);
        delay(50);
        digitalWrite(D3, HIGH);
        delay(50);
        digitalWrite(D3, LOW);
        delay(50);
      }
    } else {
      Serial.println("Unable to connect");
    }
    */

    lastCardReadTime = millis();
  }

  String data;

  while (Serial0.available() > 9) {
    data = readStackmatString();
  }

  if (data.length() >= 8) {
    ParseTimerData(data);
  }

  isConnected = millis() - lastUpdated < STACKMAT_TIMER_TIMEOUT;

  if (!isConnected) {
    //Serial.println("Timer is disconnected! Make sure it is connected and turned on.");
    //NVIC_SystemReset();

    /*
    digitalWrite(LED_BUILTIN, HIGH);
    delay(50);
    digitalWrite(LED_BUILTIN, LOW);
    delay(50);
    */
  }

  if (currentState != lastState) {
    switch (currentState) {
      case ST_Stopped:
        Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
        finishedSolveTime = timerTime;
        break;
      case ST_Reset:
        Serial.println("Timer is reset!");
        break;
      case ST_Running:
        solveSessionId++;

        Serial.println("Solve started!");
        Serial.printf("Solve session ID: %i\n", solveSessionId);
        break;
      default:
        break;
    }
  }

  if (currentState == ST_Running) {
    Serial.printf("%i:%02i.%03i\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
  }

  lastState = currentState;
}

void webSocketEvent(WStype_t type, uint8_t *payload, size_t length) {
    if (type == WStype_TEXT) {
        String str = (char *)payload;
    Serial.printf("Received message: %s\n", str.c_str());
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
