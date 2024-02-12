const http = require('http');

const people = [
    {
        id: 1,
        cardId: 3004425529,
        registrantId: 1,
        name: "Filip Sciurka",
        wcaId: "2020FILSCI",
        countryIso2: "PL",
        gender: "m"
    },
    {
        id: 2,
        cardId: 2156233370,
        registrantId: 2,
        name: "Kim Joon",
        wcaId: "2019KIMJO69",
        countryIso2: "CN",
        gender: "m"
    }
];

const requestListener = function(req, res) {
    console.log(req.method, req.url);
    let splitUrl = req.url.split('/');

    if (req.url.startsWith('/person/card/') && splitUrl.length === 4) {
        let cardId = req.url.split('/')[3];
        if (isNaN(cardId)) {
            res.writeHead(400);
            res.end('Card ID must be a number');
            return;
        }

        let person = people.find(person => person.cardId === parseInt(cardId));
        if (person === undefined) {
            res.writeHead(404);
            res.end('Person not found');
            return;
        }

        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify(person));
    } else if (req.url === '/result/enter') {
        let body = '';
        req.on('data', chunk => {
            body += chunk.toString();
        });
        req.on('end', () => {
            let result = JSON.parse(body);
            let competitor = people.find(person => person.cardId === result.competitorId);
            let judge = people.find(person => person.cardId === result.judgeId);

            if (competitor === undefined || judge === undefined) {
                res.writeHead(400, { 'Content-Type': 'application/json' });
                res.end(JSON.stringify({ message: 'Competitor or judge not found' }));
                return;
            }

            console.log(result);
            res.writeHead(200);
            res.end('');
        });
    } else {
        res.writeHead(404);
        res.end('Not Found');
    }
};

const server = http.createServer(requestListener);
server.listen(5000, "localhost", () => {
    console.log('Server is running on http://localhost:5000');
});
