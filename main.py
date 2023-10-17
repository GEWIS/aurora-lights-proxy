from stupidArtnet import StupidArtnet
import time
import signal
import requests
import socketio

# Global settings
target_ip = '169.254.0.2'		# typically in 2.x or 10.x range
universe = 0 										# see docs
packet_size = 512								# it is not necessary to send whole universe
# End global settings


# Stop thread when necessary
running = True
def stop_execution(signum, frame):
    global running
    running = False


signal.signal(signal.SIGINT, stop_execution)


def pad_array(arr, desired_length):
    current_length = len(arr)

    if current_length >= desired_length:
        # No need to pad, return the original array
        return arr
    else:
        # Calculate the number of zeros to pad
        num_zeros = desired_length - current_length

        # Create a new array with zeros and concatenate it with the original array
        padded_array = arr + [0] * num_zeros

        return padded_array


def main_thread():
    global running
    url = 'http://localhost:3000/auth/mock'
    result = requests.post(url)
    cookie = result.cookies.get('connect.sid')
    a = StupidArtnet(target_ip, universe, packet_size, 30, True, True)

    sio = socketio.Client()
    sio.connect('http://localhost:3000', headers={'cookie_development': 'connect.sid=' + cookie},
                namespaces=['/lights'])

    a.blackout()
    a.start()

    print('Start listening...')

    @sio.event(namespace='/lights')
    def dmx_packet(packet):
        a.set(pad_array(packet + packet + packet + packet + packet, packet_size)[0:packet_size])

    while running:
        time.sleep(1)

    a.stop()
    sio.disconnect()


connected = False

while running:
    try:
        print('Connecting...')
        main_thread()
    except:
        print('Something went wrong. Try again...')
        time.sleep(5)
