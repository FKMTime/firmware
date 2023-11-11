#include <Arduino.h>
#include <WiFi.h>

const char* ssid = "EZ";
const char* password = "WOW";

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
unsigned long lastUpdated = 0;
unsigned long timerTime = 0;
bool isConnected = false;

void setup() {
  Serial.begin(115200);
  Serial0.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, -1, 255, true);
  //stackmatSerial.begin(STACKMAT_TIMER_BAUD_RATE);

  //pinMode(LED_BUILTIN, OUTPUT);
  WiFi.begin(ssid, password);
  while (WiFi.status() != WL_CONNECTED) {
        delay(500);
        Serial.print(".");
    }

    Serial.println("");
    Serial.println("WiFi connected");
    Serial.println("IP address: ");
    Serial.println(WiFi.localIP());
}

void loop() {
  String data;

  while (Serial0.available() > 9) {
    data = readStackmatString();
  }

  if (data.length() >= 8) {
    ParseTimerData(data);
  }

  isConnected = millis() - lastUpdated < STACKMAT_TIMER_TIMEOUT;

  if (!isConnected) {
    Serial.println("Timer is disconnected! Make sure it is connected and turned on.");
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
        break;
      case ST_Reset:
        Serial.println("Timer is reset!");
        break;
      case ST_Running:
        Serial.println("GO!");

        break;
      default:
        break;
    }
  }

  if (currentState == ST_Running) {
    Serial.printf("%i:%02i.%03i\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
  }

  lastState = currentState;
  delay(10);
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
